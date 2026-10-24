//! The menu, web assets and live order ledger all live inside this component.
use serde_json::{Value, json};
use std::sync::Mutex;

wit_bindgen::generate!({ world: "pizzeria", path: "wit" });
use exports::agent::web::handler::{Guest as WebGuest, Request, Response};
use exports::restaurant::pizzeria::service::{
    Guest as RestaurantGuest, Menu, Order, OrderError, OrderStatus, Pizza,
};

struct Restaurant;
struct Entry {
    request_id: String,
    order: Order,
}
static ORDERS: Mutex<Vec<Entry>> = Mutex::new(Vec::new());

fn menu() -> Menu {
    let pizzas = [
        (
            "hawaii",
            "Hawaii",
            "San Marzano tomato, fior di latte, cooked ham, pineapple. Your sunny-side favorite.",
            1350,
            false,
        ),
        (
            "berghain",
            "Berghain",
            "Black dough, smoked fior di latte, roasted mushrooms, black garlic, chilli honey.",
            1650,
            true,
        ),
        (
            "checkpoint-gpt",
            "Checkpoint GPT",
            "San Marzano tomato, mozzarella, spicy salami, roasted peppers, chilli oil. Your next slice requires approval.",
            1550,
            false,
        ),
        (
            "kreuzberg-context",
            "Kreuzberg Context",
            "Roasted aubergine, feta, cherry tomatoes, tahini drizzle, parsley. A whole neighbourhood in one context window.",
            1450,
            true,
        ),
        (
            "token-currywurst",
            "Token Currywurst",
            "Curry tomato sauce, bratwurst, mozzarella, crispy onions. Berlin's favourite street food, tokenised.",
            1550,
            false,
        ),
        (
            "merge-conflict",
            "The Merge Conflict",
            "Tomato and mozzarella. Half pineapple and ham, half spicy salami and jalapenos. Resolve it one slice at a time.",
            1650,
            false,
        ),
        (
            "kitkat",
            "KitKat",
            "Pink beetroot cream, mozzarella, feta, pickled red onions, chilli honey. No dress code. Extra chilli.",
            1550,
            true,
        ),
        (
            "about-blank",
            "about blank",
            "A white pizza with ricotta, mozzarella, roasted potato, rosemary and lemon zest. Start with a blank canvas.",
            1450,
            true,
        ),
    ]
    .into_iter()
    .map(|(id, name, description, price_cents, vegetarian)| Pizza {
        id: id.into(),
        name: name.into(),
        description: description.into(),
        price_cents,
        vegetarian,
    })
    .collect();
    Menu {
        restaurant: "Forno Pizza".into(),
        currency: "EUR".into(),
        pizzas,
    }
}
fn order_json(order: &Order) -> Value {
    json!({"id": order.id, "pizza_id": order.pizza_id, "pizza_name": order.pizza_name,
        "quantity": order.quantity, "table": order.table, "total_cents": order.total_cents, "status": "received"})
}
impl RestaurantGuest for Restaurant {
    fn get_menu() -> Menu {
        menu()
    }
    fn place_order(
        pizza_id: String,
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
        let pizza = menu()
            .pizzas
            .into_iter()
            .find(|p| p.id == pizza_id)
            .ok_or(OrderError::UnknownPizza)?;
        let mut orders = ORDERS.lock().unwrap();
        if let Some(entry) = orders.iter().find(|o| o.request_id == request_id) {
            if entry.order.pizza_id == pizza_id
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
            pizza_id,
            pizza_name: pizza.name,
            quantity,
            table,
            total_cents: pizza.price_cents * quantity,
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
                body["pizza_id"].as_str(),
                body["quantity"]
                    .as_u64()
                    .and_then(|n| u32::try_from(n).ok()),
                body["table"].as_u64().and_then(|n| u32::try_from(n).ok()),
                body["request_id"].as_str(),
            );
            let (Some(pizza), Some(quantity), Some(table), Some(request_id)) = inputs else {
                return json_response(
                    400,
                    json!({"error": "Provide pizza_id, integer quantity and table, and request_id"}),
                );
            };
            return match <Self as RestaurantGuest>::place_order(
                pizza.into(),
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
            "/pizza.svg" => response(
                200,
                "image/svg+xml",
                include_bytes!("../../pizza.svg").to_vec(),
            ),
            "/api/menu" => {
                let menu = menu();
                json_response(
                    200,
                    json!({"restaurant": menu.restaurant, "currency": menu.currency, "pizzas": menu.pizzas.iter().map(|p| json!({"id": p.id, "name": p.name, "description": p.description, "price_cents": p.price_cents, "vegetarian": p.vegetarian})).collect::<Vec<_>>()}),
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
