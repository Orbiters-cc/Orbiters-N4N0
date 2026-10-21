//! Orb, the Orbiters companion (contracts `Docs/orb.md`): what Orbiters desktop needs to drive the Orb prop on the
//! avatar and to talk to Orb's assistant, without threads of its own (the desktop's controller runs it).

pub mod assistant;
pub mod context;
pub mod controls;
pub mod text;
pub mod tools;
pub mod videos;
pub mod wake;
