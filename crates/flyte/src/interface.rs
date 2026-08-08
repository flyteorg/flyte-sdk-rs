//! A task's interface, derived by `#[flyte::task]` from the fn signature.
//!
//! Two shapes, one source of truth:
//!
//! - [`TaskInterface::typed`] is the **wire** form (`TypedInterface`, variables
//!   sorted by key) used when recording actions.
//! - [`TaskInterface::to_json`] is the **descriptor**, printed by
//!   `<binary> describe-interface` and consumed by the launcher to build a
//!   Python `NativeInterface`. It preserves declaration order, so a launcher can
//!   bind arguments positionally.
//!
//! The point is that nothing outside the Rust source ever restates the
//! signature: rename a param and the descriptor changes with it.

use std::fmt::Write as _;

use crate::idl::{literal_type, LiteralType, SimpleType, TypedInterface};
use crate::types;

/// One input or output of a task.
pub struct TaskVariable {
    pub name: &'static str,
    pub literal_type: LiteralType,
    /// Always true in v0 — Rust fn params have no defaults. Carried explicitly
    /// so the launcher reads required-ness from data rather than assuming it.
    pub required: bool,
}

/// A task's inputs and outputs, in declaration order.
pub struct TaskInterface {
    pub inputs: Vec<TaskVariable>,
    pub outputs: Vec<TaskVariable>,
}

/// The descriptor's type tag for a literal type, or `None` if the type has no
/// launcher-side equivalent yet (collections, blobs, ...).
fn type_tag(lt: &LiteralType) -> Option<&'static str> {
    let simple = match lt.r#type {
        Some(literal_type::Type::Simple(s)) => s,
        _ => return None,
    };
    match SimpleType::try_from(simple).ok()? {
        SimpleType::Integer => Some("integer"),
        SimpleType::Float => Some("float"),
        SimpleType::String => Some("string"),
        SimpleType::Boolean => Some("boolean"),
        SimpleType::Struct => Some("struct"),
        _ => None,
    }
}

impl TaskInterface {
    /// The wire form. Delegates to [`types::build_typed_interface`] so the
    /// key-sorted ordering (and thus action hashing) stays byte-identical to
    /// what `#[flyte::trace]` produces.
    pub fn typed(&self) -> TypedInterface {
        fn pairs(vars: &[TaskVariable]) -> Vec<(&str, LiteralType)> {
            vars.iter()
                .map(|v| (v.name, v.literal_type.clone()))
                .collect()
        }
        types::build_typed_interface(&pairs(&self.inputs), &pairs(&self.outputs))
    }

    pub fn input(&self, name: &str) -> Option<&TaskVariable> {
        self.inputs.iter().find(|v| v.name == name)
    }

    /// The launcher-facing descriptor, one line of JSON.
    ///
    /// Written by hand rather than via serde: the only dynamic strings are
    /// variable names, which are Rust identifiers, so no escaping is required
    /// and the SDK takes no JSON dependency. Compact and byte-stable — the
    /// launcher parses it and a test pins it.
    pub fn to_json(&self, task_name: &str) -> String {
        let mut s = String::new();
        write!(
            s,
            r#"{{"flyte_interface_version":1,"task":"{task_name}","inputs":["#
        )
        .unwrap();
        for (i, v) in self.inputs.iter().enumerate() {
            if i > 0 {
                s.push(',');
            }
            write_variable(&mut s, v, true);
        }
        s.push_str(r#"],"outputs":["#);
        for (i, v) in self.outputs.iter().enumerate() {
            if i > 0 {
                s.push(',');
            }
            write_variable(&mut s, v, false);
        }
        s.push_str("]}");
        s
    }
}

/// An unmappable type is reported rather than guessed at: the launcher raises on
/// `"unsupported"`, which beats silently binding the wrong Python type.
fn write_variable(s: &mut String, v: &TaskVariable, with_required: bool) {
    write!(s, r#"{{"name":"{}","type":""#, v.name).unwrap();
    match type_tag(&v.literal_type) {
        Some(tag) => write!(s, r#"{tag}""#).unwrap(),
        None => write!(
            s,
            r#"unsupported","detail":"{}""#,
            // Debug output of a prost message contains no quotes for the shapes
            // that reach here (collection/blob wrappers), but strip any anyway.
            format!("{:?}", v.literal_type).replace('"', "'")
        )
        .unwrap(),
    }
    if with_required {
        write!(s, r#","required":{}"#, v.required).unwrap();
    }
    s.push('}');
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{struct_literal_type, FlyteType as _};

    fn var(name: &'static str, literal_type: LiteralType) -> TaskVariable {
        TaskVariable {
            name,
            literal_type,
            required: true,
        }
    }

    #[test]
    fn descriptor_keeps_declaration_order_while_wire_form_sorts() {
        let iface = TaskInterface {
            inputs: vec![
                var("x", i64::literal_type()),
                var("about", String::literal_type()),
            ],
            outputs: vec![var("o0", String::literal_type())],
        };

        // Descriptor: declaration order (x before about).
        let json = iface.to_json("t");
        let x_at = json.find(r#""name":"x""#).unwrap();
        let about_at = json.find(r#""name":"about""#).unwrap();
        assert!(x_at < about_at, "{json}");

        // Wire form: key-sorted (about before x), matching Python.
        let typed = iface.typed();
        let keys: Vec<_> = typed
            .inputs
            .unwrap()
            .variables
            .into_iter()
            .map(|v| v.key)
            .collect();
        assert_eq!(keys, vec!["about", "x"]);
    }

    #[test]
    fn every_supported_type_has_a_tag() {
        assert_eq!(type_tag(&i64::literal_type()), Some("integer"));
        assert_eq!(type_tag(&i32::literal_type()), Some("integer"));
        assert_eq!(type_tag(&f64::literal_type()), Some("float"));
        assert_eq!(type_tag(&f32::literal_type()), Some("float"));
        assert_eq!(type_tag(&String::literal_type()), Some("string"));
        assert_eq!(type_tag(&bool::literal_type()), Some("boolean"));
        assert_eq!(type_tag(&struct_literal_type()), Some("struct"));
    }

    #[test]
    fn unmappable_type_is_reported_not_guessed() {
        let unmappable = LiteralType {
            r#type: Some(literal_type::Type::Simple(SimpleType::Binary as i32)),
            ..Default::default()
        };
        assert_eq!(type_tag(&unmappable), None);
        let json = TaskInterface {
            inputs: vec![var("b", unmappable)],
            outputs: vec![],
        }
        .to_json("t");
        assert!(json.contains(r#""type":"unsupported""#), "{json}");
        assert!(json.contains(r#""detail""#), "{json}");
    }

    #[test]
    fn no_arg_no_output_task_is_still_valid_json() {
        let json = TaskInterface {
            inputs: vec![],
            outputs: vec![],
        }
        .to_json("nothing");
        assert_eq!(
            json,
            r#"{"flyte_interface_version":1,"task":"nothing","inputs":[],"outputs":[]}"#
        );
    }
}
