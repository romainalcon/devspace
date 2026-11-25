//! IPC protocol between the DevSpace daemon and its clients.
//!
//! The daemon is the single source of truth; the CLI and the desktop
//! application talk to it exclusively through the messages defined here.
//!
//! **Status:** not implemented yet. The protocol (messages, framing,
//! versioning, transport over Unix domain sockets / Windows named pipes and
//! peer authentication) is scheduled for milestone M1. This crate exposes no
//! API in M0 on purpose.
