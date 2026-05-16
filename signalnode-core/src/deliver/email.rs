use lettre::{
    message::header::ContentType, AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor,
};

use super::DeliveryError;

pub struct SmtpConfig {
    pub host: String,
    pub port: u16,
    pub user: String,
    pub pass: String,
    pub from: String,
}

pub fn build_email_message(
    from: &str,
    to: &str,
    subject: &str,
    body: &str,
) -> Result<Message, Box<dyn std::error::Error + Send + Sync>> {
    let msg = Message::builder()
        .from(from.parse()?)
        .to(to.parse()?)
        .subject(subject)
        .header(ContentType::TEXT_PLAIN)
        .body(body.to_string())?;
    Ok(msg)
}

pub async fn deliver_email(
    config: &SmtpConfig,
    to: &str,
    subject: &str,
    body: &str,
) -> Result<(), DeliveryError> {
    let msg = build_email_message(&config.from, to, subject, body)
        .map_err(DeliveryError::Email)?;

    let transport = AsyncSmtpTransport::<Tokio1Executor>::relay(&config.host)
        .map_err(|e| DeliveryError::Email(e.into()))?
        .port(config.port)
        .credentials(lettre::transport::smtp::authentication::Credentials::new(
            config.user.clone(),
            config.pass.clone(),
        ))
        .build();

    transport.send(msg).await.map_err(|e| DeliveryError::Email(e.into()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_email_message_succeeds_with_valid_addresses() {
        let msg = build_email_message(
            "from@example.com",
            "to@example.com",
            "Incident opened for monitor \"My Monitor\"",
            "An incident was opened for monitor \"My Monitor\" at 2026-05-16T00:00:00Z.",
        );
        assert!(msg.is_ok());
    }

    #[test]
    fn build_email_message_fails_with_invalid_from_address() {
        let msg = build_email_message("not-an-email", "to@example.com", "Subject", "Body");
        assert!(msg.is_err());
    }

    #[test]
    fn build_email_message_fails_with_invalid_to_address() {
        let msg = build_email_message("from@example.com", "not-an-email", "Subject", "Body");
        assert!(msg.is_err());
    }
}
