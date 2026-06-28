use wit_bindgen::FutureReader;

use crate::exports::astrobox::psys_plugin::{
    event_v3 as event,
    event_v3::EventType,
    lifecycle,
};

pub mod logger;
pub mod resources;
pub mod ui;

wit_bindgen::generate!({
    path: "wit",
    world: "psys-world-v3",
    generate_all,
});

struct MyPlugin;

impl event::Guest for MyPlugin {
    fn on_event(event_type: EventType, event_payload: _rt::String) -> FutureReader<String> {
        tracing::info!(
            "on_event enter: type={event_type:?}, payload_len={}",
            event_payload.len()
        );

        match event_type {
            EventType::PluginMessage => {
                tracing::info!("plugin-message payload: {}", event_payload);
            }
            EventType::InterconnectMessage => {}
            EventType::DeviceAction => {}
            EventType::ProviderAction => {
                tracing::info!("provider-action payload: {}", event_payload);
            }
            EventType::DeeplinkAction => {}
            EventType::TransportPacket => {}
            EventType::Timer => {}
        };

        tracing::info!("on_event dispatch begin: type={event_type:?}");
        let response = match event_type {
            EventType::PluginMessage => String::new(),
            EventType::ProviderAction => resources::handle_provider_action(&event_payload),
            _ => String::new(),
        };
        tracing::info!(
            "on_event dispatch done: type={event_type:?}, response_len={}, preview={}",
            response.len(),
            response.chars().take(200).collect::<String>()
        );
        immediate_string_logged(format!("on_event::{event_type:?}"), response)
    }

    fn on_ui_event_v3(
        event_id: _rt::String,
        event: event::Event,
        event_payload: _rt::String,
    ) -> FutureReader<_rt::String> {
        ui::ui_event_processor(event, &event_id, &event_payload);
        immediate_string_logged("on_ui_event_v3".to_string(), String::new())
    }

    fn on_ui_render(
        element_id: _rt::String,
    ) -> wit_bindgen::rt::async_support::FutureReader<()> {
        ui::render_main_ui(&element_id);
        immediate_unit_logged("on_ui_render".to_string())
    }

    fn on_card_render(
        _card_id: _rt::String,
    ) -> wit_bindgen::rt::async_support::FutureReader<()> {
        immediate_unit_logged("on_card_render".to_string())
    }
}

fn immediate_string_logged(context: String, value: String) -> FutureReader<String> {
    let (writer, reader) = wit_future::new(String::new);
    tracing::info!(
        "future resolve scheduled: context={}, response_len={}, preview={}",
        context,
        value.len(),
        value.chars().take(200).collect::<String>()
    );
    wit_bindgen::spawn(async move {
        tracing::info!("future write begin: context={context}");
        match writer.write(value).await {
            Ok(_) => tracing::info!("future write success: context={context}"),
            Err(_) => tracing::error!("future write failed: context={context}"),
        }
    });
    reader
}

fn immediate_unit_logged(context: String) -> wit_bindgen::rt::async_support::FutureReader<()> {
    let (writer, reader) = wit_future::new::<()>(|| ());
    tracing::info!("future resolve scheduled: context={context}, response_len=0, preview=");
    wit_bindgen::spawn(async move {
        tracing::info!("future write begin: context={context}");
        match writer.write(()).await {
            Ok(_) => tracing::info!("future write success: context={context}"),
            Err(_) => tracing::error!("future write failed: context={context}"),
        }
    });
    reader
}

impl lifecycle::Guest for MyPlugin {
    fn on_load() -> () {
        logger::init();
        tracing::info!("loading GiveMeFive community provider");
        let result = wit_bindgen::block_on(
            astrobox::psys_host::register::register_provider(
                resources::PROVIDER_NAME,
                astrobox::psys_host::register::ProviderType::Custom,
            )
            .into_future(),
        );
        match result {
            Ok(()) => tracing::info!("registered provider {}", resources::PROVIDER_NAME),
            Err(()) => tracing::error!("failed to register provider {}", resources::PROVIDER_NAME),
        }
    }
}

export!(MyPlugin);
