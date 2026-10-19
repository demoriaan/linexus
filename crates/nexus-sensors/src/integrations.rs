use crate::{IftttWebhookConfig, MqttPublishConfig, SensorCard};
use rumqttc::{Client as MqttClient, MqttOptions, QoS};
use serde::Serialize;
use std::time::Duration;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct IftttRequest {
    pub url: String,
    pub body: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MqttMessage {
    pub host: String,
    pub port: u16,
    pub topic: String,
    pub payload: String,
    pub retain: bool,
    pub qos: u8,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MqttDiscoveryMessage {
    pub topic: String,
    pub payload: serde_json::Value,
    pub retain: bool,
    pub qos: u8,
}

pub fn build_ifttt_request(config: &IftttWebhookConfig, card: &SensorCard) -> IftttRequest {
    IftttRequest {
        url: format!(
            "https://maker.ifttt.com/trigger/{}/with/key/{}",
            config.event, config.key
        ),
        body: serde_json::json!({
            "value1": render_integration_template(&config.value1_template, card),
            "value2": render_integration_template(&config.value2_template, card),
            "value3": render_integration_template(&config.value3_template, card),
        }),
    }
}

pub fn send_ifttt_webhook(
    config: &IftttWebhookConfig,
    card: &SensorCard,
) -> Result<IftttRequest, String> {
    let request = build_ifttt_request(config, card);
    if !request.url.starts_with("https://") {
        return Err(format!("{} is not an https address", request.url));
    }
    let output = std::process::Command::new("curl")
        .args([
            "--fail",
            "--silent",
            "--show-error",
            "--proto",
            "=https",
            "--max-time",
            "8",
            "--header",
            "Content-Type: application/json",
            "--data",
            &request.body.to_string(),
            // The address (with the user's key in it) is never an option.
            "--",
            &request.url,
        ])
        .output()
        .map_err(|error| error.to_string())?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).into_owned());
    }
    Ok(request)
}

pub fn build_mqtt_message(config: &MqttPublishConfig, card: &SensorCard) -> MqttMessage {
    MqttMessage {
        host: config.host.clone(),
        port: config.port,
        topic: render_integration_template(&config.topic, card),
        payload: render_integration_template(&config.payload_template, card),
        retain: config.retain,
        qos: config.qos,
    }
}

pub fn build_home_assistant_mqtt_discovery(
    config: &MqttPublishConfig,
) -> Option<MqttDiscoveryMessage> {
    if !config.home_assistant_discovery {
        return None;
    }
    let object_id = if config.discovery_object_id.trim().is_empty() {
        &config.id
    } else {
        &config.discovery_object_id
    };
    let device_id = if config.discovery_device_id.trim().is_empty() {
        "linexus"
    } else {
        &config.discovery_device_id
    };
    let prefix = config.discovery_prefix.trim_matches('/');
    let topic = format!("{prefix}/sensor/{device_id}/{object_id}/config");
    Some(MqttDiscoveryMessage {
        topic,
        retain: true,
        qos: config.qos,
        payload: serde_json::json!({
            "name": config.discovery_name,
            "unique_id": format!("{device_id}_{object_id}"),
            "state_topic": config.topic,
            "json_attributes_topic": config.topic,
            "value_template": config.discovery_value_template,
            "icon": "mdi:monitor-dashboard",
            "device": {
                "identifiers": [device_id],
                "name": "Linexus",
                "manufacturer": "Linexus",
                "model": "Corsair iCUE Nexus companion",
            },
        }),
    })
}

pub fn publish_mqtt_message(
    config: &MqttPublishConfig,
    card: &SensorCard,
) -> Result<MqttMessage, String> {
    let message = build_mqtt_message(config, card);
    let mut options = MqttOptions::new(&config.client_id, &config.host, config.port);
    options.set_keep_alive(Duration::from_secs(5));
    if !config.username.is_empty() {
        options.set_credentials(&config.username, &config.password);
    }
    let (client, mut connection) = MqttClient::new(options, 10);
    if let Some(discovery) = build_home_assistant_mqtt_discovery(config) {
        client
            .publish(
                &discovery.topic,
                mqtt_qos(discovery.qos),
                discovery.retain,
                discovery.payload.to_string().as_bytes(),
            )
            .map_err(|error| error.to_string())?;
    }
    client
        .publish(
            &message.topic,
            mqtt_qos(config.qos),
            config.retain,
            message.payload.as_bytes(),
        )
        .map_err(|error| error.to_string())?;
    match connection.recv_timeout(Duration::from_secs(8)) {
        Ok(Ok(_)) => Ok(message),
        Ok(Err(error)) => Err(error.to_string()),
        Err(error) => Err(format!("{error:?}")),
    }
}

fn mqtt_qos(qos: u8) -> QoS {
    match qos {
        0 => QoS::AtMostOnce,
        2 => QoS::ExactlyOnce,
        _ => QoS::AtLeastOnce,
    }
}

fn render_integration_template(template: &str, card: &SensorCard) -> String {
    template
        .replace("{id}", &card.id)
        .replace("{module}", &card.module)
        .replace("{kind}", &card.kind)
        .replace("{source}", &card.source)
        .replace("{title}", &card.title)
        .replace("{url}", card.url.as_deref().unwrap_or(""))
        .replace("{status}", card.status.as_deref().unwrap_or(""))
}
