//! Lint readers: likely bugs.

pub mod ast;
pub mod duplicate_registration;
pub mod duplicate_require;
pub mod duplicate_table_key;
pub mod extended_syntax;
pub mod format_argument_count;
pub mod global_assignment;
pub mod libraries;
pub mod nesting;
pub mod non_literal_assert_message;
pub mod redefined_label;
pub mod redefined_local;
pub mod remove_in_forward_loop;
pub mod scopes;
pub mod self_assignment;
pub mod shadowed_library;
pub mod syntax;
pub mod unbalanced_assignment;
pub mod unchecked_entity_in_callback;
pub mod undefined_doc_param;
pub mod unknown_doc_tag;
pub mod unreachable_code;
pub mod unused_self;
pub mod unused_variable;
