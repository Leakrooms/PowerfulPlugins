use wit_bindgen::FutureReader;

use crate::exports::astrobox::psys_plugin::{event_v3 as event, event_v3::EventType, lifecycle};

pub mod logger;
pub mod resources;

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

        let response = match event_type {
            EventType::ProviderAction => {
                tracing::info!("provider-action payload: {}", event_payload);
                resources::handle_provider_action(&event_payload)
            }
            _ => String::new(),
        };

        tracing::info!(
            "on_event dispatch done: type={event_type:?}, response_len={}",
            response.len()
        );
        immediate_string(response)
    }

    fn on_ui_event_v3(
        _event_id: _rt::String,
        _event: event::Event,
        _event_payload: _rt::String,
    ) -> FutureReader<_rt::String> {
        immediate_string(String::new())
    }

    fn on_ui_render(_element_id: _rt::String) -> FutureReader<()> {
        immediate_unit()
    }

    fn on_card_render(_card_id: _rt::String) -> FutureReader<()> {
        immediate_unit()
    }
}

fn immediate_string(value: String) -> FutureReader<String> {
    let (writer, reader) = wit_future::new(String::new);
    wit_bindgen::spawn(async move {
        let _ = writer.write(value).await;
    });
    reader
}

fn immediate_unit() -> FutureReader<()> {
    let (writer, reader) = wit_future::new::<()>(|| ());
    wit_bindgen::spawn(async move {
        let _ = writer.write(()).await;
    });
    reader
}

impl lifecycle::Guest for MyPlugin {
    fn on_load() -> () {
        logger::init();
        tracing::info!("loading qingwear community provider");
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
