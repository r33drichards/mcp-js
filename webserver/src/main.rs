#[macro_use]
extern crate rocket;

use dotenv::dotenv;

use crate::v8::initialize_v8;

use rocket_okapi::settings::UrlObject;

use rocket_okapi::swagger_ui::make_swagger_ui;
use rocket_okapi::{openapi_get_routes, rapidoc::*, swagger_ui::*};


use std::env;


mod error;
mod handlers;
mod guards;
mod v8;

// let id = Uuid::new_v4();


#[rocket::main]
async fn main() {
    dotenv().ok();
    initialize_v8();
    
    let args: Vec<String> = env::args().collect();
    if args.contains(&"--print-openapi".to_string()) {
        let settings = rocket_okapi::settings::OpenApiSettings::new();
        let spec = rocket_okapi::openapi_spec![
            handlers::javascript::javascript,
            handlers::log::log,
        ](&settings);
        println!("{}", serde_json::to_string_pretty(&spec).unwrap());
        return;
    }


    let _ = rocket::build()
        .configure(rocket::Config {
            address: "0.0.0.0".parse().expect("valid IP address"),
            port: 8000,
            ..rocket::Config::default()
        })
        .mount(
            "/",
            openapi_get_routes![
                handlers::log::log,
                handlers::javascript::javascript,
            ],
        )
        .mount(
            "/swagger-ui/",
            make_swagger_ui(&SwaggerUIConfig {
                url: "../openapi.json".to_owned(),
                ..Default::default()
            }),
        )
        .mount(
            "/rapidoc/",
            make_rapidoc(&RapiDocConfig {
                general: GeneralConfig {
                    spec_urls: vec![UrlObject::new("General", "../openapi.json")],
                    ..Default::default()
                },
                hide_show: HideShowConfig {
                    allow_spec_url_load: false,
                    allow_spec_file_load: false,
                    ..Default::default()
                },
                ..Default::default()
            }),
        )
        .launch()
        .await;
}
