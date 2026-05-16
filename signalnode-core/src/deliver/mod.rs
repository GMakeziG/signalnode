pub mod email;
pub mod webhook;

pub use email::{build_email_message, deliver_email, SmtpConfig};
pub use webhook::deliver_webhook;

#[derive(Debug)]
pub enum DeliveryError {
    Http(reqwest::Error),
    HttpStatus(u16),
    Email(Box<dyn std::error::Error + Send + Sync>),
}

impl std::fmt::Display for DeliveryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DeliveryError::Http(e) => write!(f, "HTTP error: {e}"),
            DeliveryError::HttpStatus(s) => write!(f, "non-success HTTP status: {s}"),
            DeliveryError::Email(e) => write!(f, "email error: {e}"),
        }
    }
}

impl std::error::Error for DeliveryError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            DeliveryError::Http(e) => Some(e),
            DeliveryError::Email(e) => Some(e.as_ref()),
            DeliveryError::HttpStatus(_) => None,
        }
    }
}
