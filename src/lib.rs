//! A RuboCop-like linter and autocorrector for GLua, the Lua dialect used by Garry's Mod.
//!
//! Source files are lexed into [`token::Token`]s, wrapped in a [`source::Source`] and handed to
//! every enabled [`reader::Reader`]. Readers report [`offense::Offense`]s, optionally with a fix
//! that the [`runner`] applies through the [`corrector`].

pub mod cli;
pub mod config;
pub mod corrector;
pub mod directive;
pub mod formatter;
pub mod lexer;
pub mod offense;
pub mod reader;
pub mod readers;
pub mod runner;
pub mod source;
pub mod testing;
pub mod token;

pub use yaml_rust2::Yaml;
