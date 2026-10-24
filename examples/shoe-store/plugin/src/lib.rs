//! One retained component holds stock, reservations, and the storefront.
use serde_json::{Value, json};
use std::sync::Mutex;

wit_bindgen::generate!({world: "shoe-store", path: "wit"});
use exports::agent::web::handler::{Guest as WebGuest, Request, Response};
use exports::berlin::shoes::inventory::{
    Catalog, Guest, Product, Reservation, ReservationError as Error, ReservationStatus as Status,
    SizeStock,
};
struct Shop;
struct Entry {
    request_id: String,
    reservation: Reservation,
}
static RESERVATIONS: Mutex<Vec<Entry>> = Mutex::new(Vec::new());

fn products() -> Vec<Product> {
    [
        (
            "ringbahn-runner",
            "Ringbahn Runner",
            "Suede and mesh in concrete grey and signal yellow. Ready for one more lap.",
            12500,
        ),
        (
            "kreuzberg-canvas",
            "Kreuzberg Canvas",
            "Off-white canvas, cobalt stitching, gum sole. From the canal to the courtyard.",
            7900,
        ),
        (
            "afterhours-high-top",
            "Afterhours High-Top",
            "All-black high-top with a reflective heel. Built for the long way home.",
            14900,
        ),
        (
            "tempelhof-trainer",
            "Tempelhof Trainer",
            "Sage green retro trainer with a cushioned sole. Plenty of room to roam.",
            9900,
        ),
    ]
    .into_iter()
    .map(|(id, name, description, price_cents)| Product {
        id: id.into(),
        name: name.into(),
        description: description.into(),
        price_cents,
        sizes: (36..=46)
            .map(|size| SizeStock {
                size,
                available: if size == 41 { 1 } else { 2 },
            })
            .collect(),
    })
    .collect()
}
fn reservation_json(r: &Reservation) -> Value {
    json!({"id":r.id,"product_id":r.product_id,"product_name":r.product_name,"size":r.size,"price_cents":r.price_cents,"status":if matches!(r.status, Status::Reserved){"reserved"}else{"cancelled"}})
}
impl Guest for Shop {
    fn get_catalog() -> Catalog {
        let entries = RESERVATIONS.lock().unwrap();
        let mut products = products();
        for p in &mut products {
            for stock in &mut p.sizes {
                let reserved = entries
                    .iter()
                    .filter(|e| {
                        e.reservation.product_id == p.id
                            && e.reservation.size == stock.size
                            && matches!(e.reservation.status, Status::Reserved)
                    })
                    .count() as u32;
                stock.available = stock.available.saturating_sub(reserved);
            }
        }
        Catalog {
            shop: "Kiez Kicks".into(),
            currency: "EUR".into(),
            products,
        }
    }
    fn reserve(product_id: String, size: u32, request_id: String) -> Result<Reservation, Error> {
        if !(8..=64).contains(&request_id.len())
            || !request_id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
        {
            return Err(Error::InvalidRequestId);
        }
        let product = products()
            .into_iter()
            .find(|p| p.id == product_id)
            .ok_or(Error::UnknownProduct)?;
        let stock = product
            .sizes
            .iter()
            .find(|s| s.size == size)
            .ok_or(Error::InvalidSize)?;
        let mut entries = RESERVATIONS.lock().unwrap();
        if let Some(entry) = entries.iter().find(|e| e.request_id == request_id) {
            if entry.reservation.product_id == product_id && entry.reservation.size == size {
                return Ok(entry.reservation.clone());
            }
            return Err(Error::RequestIdConflict);
        }
        let reserved = entries
            .iter()
            .filter(|e| {
                e.reservation.product_id == product_id
                    && e.reservation.size == size
                    && matches!(e.reservation.status, Status::Reserved)
            })
            .count() as u32;
        if reserved >= stock.available {
            return Err(Error::OutOfStock);
        }
        if entries.len() >= 256 {
            return Err(Error::CapacityReached);
        }
        let reservation = Reservation {
            id: entries.len() as u32 + 1,
            product_id,
            product_name: product.name,
            size,
            price_cents: product.price_cents,
            status: Status::Reserved,
        };
        entries.push(Entry {
            request_id,
            reservation: reservation.clone(),
        });
        Ok(reservation)
    }
    fn get_reservation(id: u32) -> Option<Reservation> {
        RESERVATIONS
            .lock()
            .unwrap()
            .iter()
            .find(|e| e.reservation.id == id)
            .map(|e| e.reservation.clone())
    }
    fn get_reservations() -> Vec<Reservation> {
        RESERVATIONS
            .lock()
            .unwrap()
            .iter()
            .map(|e| e.reservation.clone())
            .collect()
    }
    fn cancel(id: u32) -> Result<Reservation, Error> {
        let mut entries = RESERVATIONS.lock().unwrap();
        let entry = entries
            .iter_mut()
            .find(|e| e.reservation.id == id)
            .ok_or(Error::UnknownReservation)?;
        entry.reservation.status = Status::Cancelled;
        Ok(entry.reservation.clone())
    }
}
fn response(status: u16, content_type: &str, body: Vec<u8>) -> Response {
    Response {
        status,
        headers: vec![
            ("content-type".into(), content_type.into()),
            ("cache-control".into(), "no-store".into()),
        ],
        body,
    }
}
fn json_response(status: u16, value: Value) -> Response {
    response(
        status,
        "application/json",
        serde_json::to_vec(&value).unwrap(),
    )
}
fn json_input(request: &Request) -> Result<Value, Response> {
    if !request.headers.iter().any(|(k, v)| {
        k.eq_ignore_ascii_case("content-type")
            && v.split(';')
                .next()
                .is_some_and(|v| v.trim().eq_ignore_ascii_case("application/json"))
    }) {
        return Err(json_response(415, json!({"error":"Use application/json"})));
    }
    serde_json::from_slice(&request.body)
        .map_err(|_| json_response(400, json!({"error":"Invalid JSON"})))
}
fn result_json(result: Result<Reservation, Error>) -> Response {
    match result {
        Ok(r) => json_response(200, json!({"reservation":reservation_json(&r)})),
        Err(e) => json_response(
            if matches!(e, Error::OutOfStock | Error::RequestIdConflict) {
                409
            } else {
                400
            },
            json!({"error":format!("{e:?}")}),
        ),
    }
}
impl WebGuest for Shop {
    fn handle(request: Request) -> Response {
        let path = request.path.split('?').next().unwrap_or("/");
        if request.method == "POST" && (path == "/api/reserve" || path == "/api/cancel") {
            let body = match json_input(&request) {
                Ok(v) => v,
                Err(r) => return r,
            };
            if path == "/api/cancel" {
                return match body["id"].as_u64().and_then(|n| u32::try_from(n).ok()) {
                    Some(id) => result_json(Self::cancel(id)),
                    None => json_response(400, json!({"error":"Provide integer id"})),
                };
            }
            return match (
                body["product_id"].as_str(),
                body["size"].as_u64().and_then(|n| u32::try_from(n).ok()),
                body["request_id"].as_str(),
            ) {
                (Some(product), Some(size), Some(request)) => {
                    result_json(Self::reserve(product.into(), size, request.into()))
                }
                _ => json_response(
                    400,
                    json!({"error":"Provide product_id, integer size, and request_id"}),
                ),
            };
        }
        if request.method != "GET" && request.method != "HEAD" {
            return json_response(405, json!({"error":"Method not allowed"}));
        }
        match path {
            "/" | "/index.html" => response(
                200,
                "text/html; charset=utf-8",
                include_bytes!("../../index.html").to_vec(),
            ),
            "/style.css" => response(
                200,
                "text/css; charset=utf-8",
                include_bytes!("../../style.css").to_vec(),
            ),
            "/app.js" => response(
                200,
                "text/javascript; charset=utf-8",
                include_bytes!("../../app.js").to_vec(),
            ),
            "/shoe.svg" => response(
                200,
                "image/svg+xml",
                include_bytes!("../../shoe.svg").to_vec(),
            ),
            "/api/catalog" => {
                let c = Self::get_catalog();
                json_response(
                    200,
                    json!({"shop":c.shop,"currency":c.currency,"products":c.products.iter().map(|p|json!({"id":p.id,"name":p.name,"description":p.description,"price_cents":p.price_cents,"sizes":p.sizes.iter().map(|s|json!({"size":s.size,"available":s.available})).collect::<Vec<_>>()})).collect::<Vec<_>>()}),
                )
            }
            "/api/reservations" => json_response(
                200,
                json!({"reservations":Self::get_reservations().iter().map(reservation_json).collect::<Vec<_>>()}),
            ),
            _ => json_response(404, json!({"error":"Not found"})),
        }
    }
}
export!(Shop);
