//! The menu, web assets and live order ledger all live inside this component.
use serde_json::{Value, json};
use std::sync::Mutex;

wit_bindgen::generate!({ world: "vendor", path: "wit" });
use exports::agent::web::handler::{Guest as WebGuest, Request, Response};
use exports::foodcourt::vendor::service::{
    Guest as RestaurantGuest, Item, Menu, Order, OrderError, OrderStatus,
};

struct Restaurant;
struct Entry {
    request_id: String,
    order: Order,
}
static ORDERS: Mutex<Vec<Entry>> = Mutex::new(Vec::new());

fn vendor_data() -> Value {
    let source = match option_env!("FOODCOURT_VENDOR") {
        Some("ramen") => include_str!("../../vendors/ramen.json"),
        Some("spritz") => include_str!("../../vendors/spritz.json"),
        Some("taproom") => include_str!("../../vendors/taproom.json"),
        _ => include_str!("../../vendors/tacos.json"),
    };
    serde_json::from_str(source).unwrap()
}
fn menu() -> Menu {
    let data = vendor_data();
    Menu {
        restaurant: data["name"].as_str().unwrap().into(),
        category: data["category"].as_str().unwrap().into(),
        tagline: data["tagline"].as_str().unwrap().into(),
        currency: "EUR".into(),
        items: data["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| Item {
                id: p["id"].as_str().unwrap().into(),
                name: p["name"].as_str().unwrap().into(),
                description: p["description"].as_str().unwrap().into(),
                price_cents: p["price_cents"].as_u64().unwrap() as u32,
                vegetarian: p["vegetarian"].as_bool().unwrap(),
            })
            .collect(),
    }
}
fn order_json(order: &Order) -> Value {
    json!({"id": order.id, "item_id": order.item_id, "item_name": order.item_name,
        "quantity": order.quantity, "table": order.table, "total_cents": order.total_cents, "status": "received"})
}
impl RestaurantGuest for Restaurant {
    fn get_menu() -> Menu {
        menu()
    }
    fn place_order(
        item_id: String,
        quantity: u32,
        table: u32,
        request_id: String,
    ) -> Result<Order, OrderError> {
        if !(1..=5).contains(&quantity) {
            return Err(OrderError::InvalidQuantity);
        }
        if !(1..=40).contains(&table) {
            return Err(OrderError::InvalidTable);
        }
        if !(8..=64).contains(&request_id.len())
            || !request_id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
        {
            return Err(OrderError::InvalidRequestId);
        }
        let item = menu()
            .items
            .into_iter()
            .find(|p| p.id == item_id)
            .ok_or(OrderError::UnknownItem)?;
        let mut orders = ORDERS.lock().unwrap();
        if let Some(entry) = orders.iter().find(|o| o.request_id == request_id) {
            if entry.order.item_id == item_id
                && entry.order.quantity == quantity
                && entry.order.table == table
            {
                return Ok(entry.order.clone());
            }
            return Err(OrderError::RequestIdConflict);
        }
        if orders.len() >= 256 {
            return Err(OrderError::CapacityReached);
        }
        let order = Order {
            id: orders.len() as u32 + 1,
            item_id,
            item_name: item.name,
            quantity,
            table,
            total_cents: item.price_cents * quantity,
            status: OrderStatus::Received,
        };
        orders.push(Entry {
            request_id,
            order: order.clone(),
        });
        Ok(order)
    }
    fn get_order(id: u32) -> Option<Order> {
        ORDERS
            .lock()
            .unwrap()
            .iter()
            .find(|o| o.order.id == id)
            .map(|o| o.order.clone())
    }
    fn get_table_orders(table: u32) -> Result<Vec<Order>, OrderError> {
        if !(1..=40).contains(&table) {
            return Err(OrderError::InvalidTable);
        }
        Ok(ORDERS
            .lock()
            .unwrap()
            .iter()
            .filter(|o| o.order.table == table)
            .map(|o| o.order.clone())
            .collect())
    }
    fn get_open_orders() -> Vec<Order> {
        ORDERS
            .lock()
            .unwrap()
            .iter()
            .map(|entry| entry.order.clone())
            .collect()
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
impl WebGuest for Restaurant {
    fn handle(request: Request) -> Response {
        let (path, query) = request.path.split_once('?').unwrap_or((&request.path, ""));
        if request.method == "POST" && path == "/api/orders" {
            let content_type = request
                .headers
                .iter()
                .find(|(name, _)| name.eq_ignore_ascii_case("content-type"));
            if !content_type.is_some_and(|(_, value)| {
                value
                    .split(';')
                    .next()
                    .is_some_and(|v| v.trim().eq_ignore_ascii_case("application/json"))
            }) {
                return json_response(415, json!({"error": "Use application/json"}));
            }
            let Ok(body) = serde_json::from_slice::<Value>(&request.body) else {
                return json_response(400, json!({"error": "Invalid JSON"}));
            };
            let inputs = (
                body["item_id"].as_str(),
                body["quantity"]
                    .as_u64()
                    .and_then(|n| u32::try_from(n).ok()),
                body["table"].as_u64().and_then(|n| u32::try_from(n).ok()),
                body["request_id"].as_str(),
            );
            let (Some(item), Some(quantity), Some(table), Some(request_id)) = inputs else {
                return json_response(
                    400,
                    json!({"error": "Provide item_id, integer quantity and table, and request_id"}),
                );
            };
            return match <Self as RestaurantGuest>::place_order(
                item.into(),
                quantity,
                table,
                request_id.into(),
            ) {
                Ok(order) => json_response(200, json!({"order": order_json(&order)})),
                Err(error) => json_response(400, json!({"error": format!("{error:?}")})),
            };
        }
        if request.method != "GET" && request.method != "HEAD" {
            return json_response(405, json!({"error": "Method not allowed"}));
        }
        match path {
            "/back-office" | "/back-office/" => response(
                200,
                "text/html; charset=utf-8",
                include_bytes!("../../back-office.html").to_vec(),
            ),
            "/back-office.js" => response(
                200,
                "text/javascript; charset=utf-8",
                include_bytes!("../../back-office.js").to_vec(),
            ),
            "/api/open-orders" => json_response(
                200,
                json!({"orders": <Self as RestaurantGuest>::get_open_orders().iter().map(order_json).collect::<Vec<_>>()}),
            ),
            "/" | "/index.html" => response(
                200,
                "text/html; charset=utf-8",
                include_bytes!("../../index.html").to_vec(),
            ),
            "/app.js" => response(
                200,
                "text/javascript; charset=utf-8",
                include_bytes!("../../app.js").to_vec(),
            ),
            "/style.css" => response(
                200,
                "text/css; charset=utf-8",
                include_bytes!("../../style.css").to_vec(),
            ),
            "/api/menu" => {
                let menu = menu();
                json_response(
                    200,
                    json!({"restaurant": menu.restaurant, "category": menu.category, "tagline": menu.tagline, "accent": vendor_data()["accent"], "currency": menu.currency, "items": menu.items.iter().map(|p| json!({"id": p.id, "name": p.name, "description": p.description, "price_cents": p.price_cents, "vegetarian": p.vegetarian})).collect::<Vec<_>>()}),
                )
            }
            "/api/orders" => {
                let table = query
                    .split('&')
                    .filter_map(|p| p.split_once('='))
                    .find(|(key, _)| *key == "table")
                    .and_then(|(_, v)| v.parse().ok())
                    .unwrap_or(0);
                match <Self as RestaurantGuest>::get_table_orders(table) {
                    Ok(orders) => json_response(
                        200,
                        json!({"table": table, "orders": orders.iter().map(order_json).collect::<Vec<_>>()}),
                    ),
                    Err(_) => {
                        json_response(400, json!({"error": "Choose a table between 1 and 40"}))
                    }
                }
            }
            _ => {
                if let Some(id) = path
                    .strip_prefix("/api/orders/")
                    .and_then(|id| id.parse().ok())
                    && let Some(order) = <Self as RestaurantGuest>::get_order(id)
                {
                    return json_response(200, order_json(&order));
                }
                json_response(404, json!({"error": "Not found"}))
            }
        }
    }
}
export!(Restaurant);
