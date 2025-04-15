
use rocket::{Request, http::Status, outcome::Outcome};
use rocket::request::{self, FromRequest};

/// Custom request guard type that checks for the presence of a 'Debug' header.
pub struct DebugHeader;

#[rocket::async_trait]
impl<'r> FromRequest<'r> for DebugHeader {
    type Error = ();

    async fn from_request(request: &'r Request<'_>) -> request::Outcome<Self, Self::Error> {
        // Check if the 'Debug' header is present
        if request.headers().contains("Debug") {
            Outcome::Success(DebugHeader)
        } else {
            Outcome::Forward(
                Status::Ok
            )
        }
    }
}
