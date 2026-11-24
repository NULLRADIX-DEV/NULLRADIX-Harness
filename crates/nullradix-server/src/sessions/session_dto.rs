//! Antwortformat der Sessions; die Typen liegen im Fachkern (`nullradix_core::sessions::dto`),
//! damit ts-rs sie mit der Detailansicht erzeugt. Hier bleiben die bisherigen Namen.

pub use nullradix_core::sessions::dto::{
    SessionDto, SubagentDto, TicketInfo, UNKNOWN_STATUS, UNKNOWN_TITLE,
};
