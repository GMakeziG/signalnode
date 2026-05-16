use serde_json::Value;

use super::DeliveryError;

pub async fn deliver_webhook(
    client: &reqwest::Client,
    target: &str,
    payload: Value,
) -> Result<(), DeliveryError> {
    let response = client
        .post(target)
        .json(&payload)
        .send()
        .await
        .map_err(DeliveryError::Http)?;

    if response.status().is_success() {
        Ok(())
    } else {
        Err(DeliveryError::HttpStatus(response.status().as_u16()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn deliver_webhook_succeeds_on_200() {
        let mock = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .respond_with(wiremock::ResponseTemplate::new(200))
            .expect(1)
            .mount(&mock)
            .await;

        let client = reqwest::Client::new();
        let result = deliver_webhook(&client, &mock.uri(), serde_json::json!({"x": 1})).await;
        assert!(result.is_ok());
        mock.verify().await;
    }

    #[tokio::test]
    async fn deliver_webhook_fails_on_non_2xx() {
        let mock = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .respond_with(wiremock::ResponseTemplate::new(500))
            .mount(&mock)
            .await;

        let client = reqwest::Client::new();
        let result = deliver_webhook(&client, &mock.uri(), serde_json::json!({})).await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), DeliveryError::HttpStatus(500)));
    }

    #[tokio::test]
    async fn deliver_webhook_fails_on_network_error() {
        // Nothing listens on port 1 — immediate connection refused
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_millis(200))
            .build()
            .unwrap();
        let result = deliver_webhook(&client, "http://127.0.0.1:1", serde_json::json!({})).await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), DeliveryError::Http(_)));
    }
}
