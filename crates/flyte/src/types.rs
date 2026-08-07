//! Native ⇄ Flyte literal conversion.
//!
//! Mirrors the Python type engine for the v0 surface: primitives map to
//! `Literal.scalar.primitive`, structs map to msgpack bytes in
//! `Literal.scalar.binary{tag="msgpack"}` (wire-compatible with mashumaro's
//! string-keyed msgpack maps).

use crate::error::Error;
use crate::idl::{
    literal, literal_type, scalar, Binary, Inputs, Literal, LiteralType, NamedLiteral, Outputs,
    Primitive, Scalar, SimpleType, TypedInterface, Variable, VariableEntry, VariableMap,
};

pub const MSGPACK_TAG: &str = "msgpack";

/// Conversion between native Rust values and Flyte literals.
///
/// Implementation plumbing: users never implement this directly — primitives are
/// covered below and structs use `#[derive(FlyteStruct)]`.
#[doc(hidden)]
pub trait FlyteType: Sized {
    fn literal_type() -> LiteralType;
    fn to_literal(&self) -> Result<Literal, Error>;
    fn from_literal(lit: &Literal) -> Result<Self, Error>;
}

fn simple_literal_type(t: SimpleType) -> LiteralType {
    LiteralType {
        r#type: Some(literal_type::Type::Simple(t as i32)),
        ..Default::default()
    }
}

fn primitive_literal(value: crate::idl::primitive::Value) -> Literal {
    Literal {
        value: Some(literal::Value::Scalar(Box::new(Scalar {
            value: Some(scalar::Value::Primitive(Primitive { value: Some(value) })),
        }))),
        ..Default::default()
    }
}

fn primitive_of(lit: &Literal) -> Option<&crate::idl::primitive::Value> {
    match &lit.value {
        Some(literal::Value::Scalar(s)) => match &s.value {
            Some(scalar::Value::Primitive(p)) => p.value.as_ref(),
            _ => None,
        },
        _ => None,
    }
}

macro_rules! int_flyte_type {
    ($t:ty) => {
        impl FlyteType for $t {
            fn literal_type() -> LiteralType {
                simple_literal_type(SimpleType::Integer)
            }
            fn to_literal(&self) -> Result<Literal, Error> {
                Ok(primitive_literal(crate::idl::primitive::Value::Integer(
                    *self as i64,
                )))
            }
            fn from_literal(lit: &Literal) -> Result<Self, Error> {
                match primitive_of(lit) {
                    Some(crate::idl::primitive::Value::Integer(v)) => {
                        <$t>::try_from(*v).map_err(|_| {
                            Error::Type(format!(
                                "integer {v} out of range for {}",
                                stringify!($t)
                            ))
                        })
                    }
                    _ => Err(Error::Type(format!(
                        "expected integer literal for {}",
                        stringify!($t)
                    ))),
                }
            }
        }
    };
}

int_flyte_type!(i64);
int_flyte_type!(i32);

macro_rules! float_flyte_type {
    ($t:ty) => {
        impl FlyteType for $t {
            fn literal_type() -> LiteralType {
                simple_literal_type(SimpleType::Float)
            }
            fn to_literal(&self) -> Result<Literal, Error> {
                Ok(primitive_literal(crate::idl::primitive::Value::FloatValue(
                    *self as f64,
                )))
            }
            fn from_literal(lit: &Literal) -> Result<Self, Error> {
                match primitive_of(lit) {
                    Some(crate::idl::primitive::Value::FloatValue(v)) => Ok(*v as $t),
                    // Match Python's lenient int→float coercion.
                    Some(crate::idl::primitive::Value::Integer(v)) => Ok(*v as $t),
                    _ => Err(Error::Type(format!(
                        "expected float literal for {}",
                        stringify!($t)
                    ))),
                }
            }
        }
    };
}

float_flyte_type!(f64);
float_flyte_type!(f32);

impl FlyteType for String {
    fn literal_type() -> LiteralType {
        simple_literal_type(SimpleType::String)
    }
    fn to_literal(&self) -> Result<Literal, Error> {
        Ok(primitive_literal(crate::idl::primitive::Value::StringValue(
            self.clone(),
        )))
    }
    fn from_literal(lit: &Literal) -> Result<Self, Error> {
        match primitive_of(lit) {
            Some(crate::idl::primitive::Value::StringValue(v)) => Ok(v.clone()),
            _ => Err(Error::Type("expected string literal".into())),
        }
    }
}

impl FlyteType for bool {
    fn literal_type() -> LiteralType {
        simple_literal_type(SimpleType::Boolean)
    }
    fn to_literal(&self) -> Result<Literal, Error> {
        Ok(primitive_literal(crate::idl::primitive::Value::Boolean(
            *self,
        )))
    }
    fn from_literal(lit: &Literal) -> Result<Self, Error> {
        match primitive_of(lit) {
            Some(crate::idl::primitive::Value::Boolean(v)) => Ok(*v),
            _ => Err(Error::Type("expected boolean literal".into())),
        }
    }
}

/// msgpack helpers backing `#[derive(FlyteStruct)]`.
#[doc(hidden)]
pub fn struct_literal_type() -> LiteralType {
    simple_literal_type(SimpleType::Struct)
}

#[doc(hidden)]
pub fn msgpack_to_literal<T: serde::Serialize>(value: &T) -> Result<Literal, Error> {
    // to_vec_named writes string-keyed maps — the encoding mashumaro produces
    // for dataclasses, so structs round-trip across the Python SDK.
    let bytes = rmp_serde::to_vec_named(value)
        .map_err(|e| Error::Type(format!("msgpack encode failed: {e}")))?;
    Ok(Literal {
        value: Some(literal::Value::Scalar(Box::new(Scalar {
            value: Some(scalar::Value::Binary(Binary {
                value: bytes,
                tag: MSGPACK_TAG.to_string(),
            })),
        }))),
        ..Default::default()
    })
}

#[doc(hidden)]
pub fn msgpack_from_literal<T: serde::de::DeserializeOwned>(lit: &Literal) -> Result<T, Error> {
    match &lit.value {
        Some(literal::Value::Scalar(s)) => match &s.value {
            Some(scalar::Value::Binary(b)) => {
                if !b.tag.is_empty() && b.tag != MSGPACK_TAG {
                    return Err(Error::Type(format!(
                        "unsupported binary literal tag {:?} (expected {MSGPACK_TAG:?})",
                        b.tag
                    )));
                }
                rmp_serde::from_slice(&b.value)
                    .map_err(|e| Error::Type(format!("msgpack decode failed: {e}")))
            }
            _ => Err(Error::Type("expected binary (msgpack) literal".into())),
        },
        _ => Err(Error::Type("expected scalar literal".into())),
    }
}

/// Build an `Inputs` envelope preserving declaration order.
#[doc(hidden)]
pub fn build_inputs(named: Vec<(&str, Literal)>) -> Inputs {
    Inputs {
        literals: named
            .into_iter()
            .map(|(name, value)| NamedLiteral {
                name: name.to_string(),
                value: Some(value),
            })
            .collect(),
        ..Default::default()
    }
}

/// Build an `Outputs` envelope (names are `o0`, `o1`, ... by convention).
#[doc(hidden)]
pub fn build_outputs(named: Vec<(&str, Literal)>) -> Outputs {
    Outputs {
        literals: named
            .into_iter()
            .map(|(name, value)| NamedLiteral {
                name: name.to_string(),
                value: Some(value),
            })
            .collect(),
        ..Default::default()
    }
}

/// Build a `TypedInterface`; variables sorted by key like Python's
/// `types_serde.transform_native_to_typed_interface` (hash stability).
#[doc(hidden)]
pub fn build_typed_interface(
    inputs: &[(&str, LiteralType)],
    outputs: &[(&str, LiteralType)],
) -> TypedInterface {
    fn to_map(vars: &[(&str, LiteralType)]) -> VariableMap {
        let mut sorted: Vec<_> = vars.to_vec();
        sorted.sort_by(|a, b| a.0.cmp(b.0));
        VariableMap {
            variables: sorted
                .into_iter()
                .map(|(key, lt)| VariableEntry {
                    key: key.to_string(),
                    value: Some(Variable {
                        r#type: Some(lt),
                        ..Default::default()
                    }),
                })
                .collect(),
        }
    }
    TypedInterface {
        inputs: Some(to_map(inputs)),
        outputs: Some(to_map(outputs)),
    }
}

/// Extract a named output literal from an `Outputs` envelope.
#[doc(hidden)]
pub fn output_literal<'a>(outputs: &'a Outputs, name: &str) -> Result<&'a Literal, Error> {
    outputs
        .literals
        .iter()
        .find(|n| n.name == name)
        .and_then(|n| n.value.as_ref())
        .ok_or_else(|| Error::Type(format!("outputs missing {name}")))
}

/// Extract a named input literal from an `Inputs` envelope.
#[doc(hidden)]
pub fn input_literal<'a>(inputs: &'a Inputs, name: &str) -> Result<&'a Literal, Error> {
    inputs
        .literals
        .iter()
        .find(|n| n.name == name)
        .and_then(|n| n.value.as_ref())
        .ok_or_else(|| Error::Type(format!("inputs missing {name}")))
}
