//! Security readers: server code that trusts what clients send, such as net messages reaching
//! `RunString` or unescaped values in SQL queries. Code that only runs on the client (see
//! [`realm`]) is left alone by the readers about server-side handlers.

pub mod net_message_execution;
pub mod net_read_table;
pub mod realm;
pub mod sql_injection;
pub mod unchecked_console_command;
