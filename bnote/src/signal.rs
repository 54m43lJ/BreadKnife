//! Emission of the three protocol signals.
//!
//! Signal declarations live on the zbus interface in [`crate::server`] so they
//! appear in introspection; this module only performs the actual emission.

use zbus::Connection;

use crate::bus::Bus;
use crate::types::CloseReason;
use crate::{Error, INTERFACE, PATH};

pub(crate) async fn emit_closed(bus: &Bus, id: u32, reason: CloseReason) -> Result<(), Error> {
    emit_closed_on(&bus.conn, id, reason).await
}

pub(crate) async fn emit_closed_on(
    conn: &Connection,
    id: u32,
    reason: CloseReason,
) -> Result<(), Error> {
    conn.emit_signal(
        None::<&str>,
        PATH,
        INTERFACE,
        "NotificationClosed",
        &(id, reason.code()),
    )
    .await
    .map_err(|error| Error::Bus(error.to_string()))
}

pub(crate) async fn emit_action_invoked(bus: &Bus, id: u32, action_key: &str) -> Result<(), Error> {
    bus.conn
        .emit_signal(
            None::<&str>,
            PATH,
            INTERFACE,
            "ActionInvoked",
            &(id, action_key),
        )
        .await
        .map_err(|error| Error::Bus(error.to_string()))
}

pub(crate) async fn emit_activation_token(bus: &Bus, id: u32, token: &str) -> Result<(), Error> {
    bus.conn
        .emit_signal(
            None::<&str>,
            PATH,
            INTERFACE,
            "ActivationToken",
            &(id, token),
        )
        .await
        .map_err(|error| Error::Bus(error.to_string()))
}
