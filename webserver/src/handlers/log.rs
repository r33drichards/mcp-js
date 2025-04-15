
use rocket::serde::json::Json;
use rocket_okapi::openapi;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::error::OResult;

#[derive(Serialize, Deserialize, JsonSchema, Clone)]
pub struct LogInput {
    log: String,
}

#[derive(Serialize, Deserialize, JsonSchema, Clone)]
pub struct LogOutput {
}

/// Get instance ID from queue
///
/// Retrieves the next available EC2 instance ID from the queue.
#[openapi]
#[post("/log", data = "<input>")]
pub async fn log(
    input: Json<LogInput>,
) -> OResult<LogOutput>  {
    println!("Log: {}", input.log);
    Ok(Json(LogOutput{}))
}