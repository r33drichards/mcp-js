use rocket::serde::json::Json;
use rocket_okapi::openapi;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::error::OResult;
use crate::v8::eval_js;

#[derive(Serialize, Deserialize, JsonSchema, Clone)]
pub struct JavaScriptInput {
    code: String,
}

#[derive(Serialize, Deserialize, JsonSchema, Clone)]
pub struct JavaScriptOutput {
    result: String,
}

/// Evaluate JavaScript code
///
/// Evaluates the provided JavaScript code and returns the result.
#[openapi]
#[post("/javascript", data = "<input>")]
pub async fn javascript(
    input: Json<JavaScriptInput>,
) -> OResult<JavaScriptOutput> {
    let result = eval_js(&input.code).unwrap();
    Ok(Json(JavaScriptOutput { result }))
}