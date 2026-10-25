//! Lint readers: likely bugs.

pub mod ast;
pub mod duplicate_registration;
pub mod duplicate_require;
pub mod duplicate_table_key;
pub mod libraries;
pub mod nesting;
pub mod non_literal_assert_message;
pub mod redefined_label;
pub mod redefined_local;
pub mod scopes;
pub mod shadowed_library;
pub mod syntax;
pub mod unbalanced_assignment;
pub mod undefined_doc_param;
pub mod unknown_doc_tag;
pub mod unreachable_code;
pub mod unused_self;
pub mod unused_variable;
