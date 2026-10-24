//! Lossless JSON encoding for component values. Tagged wrappers preserve nested options.
use anyhow::{Context, Result, bail, ensure};
use serde_json::{Value, json};
use wasmtime::component::{Type, Val};

pub fn describe(ty: &Type) -> Value {
    match ty {
        Type::List(t) => json!({"list": describe(&t.ty())}),
        Type::Record(t) => {
            json!({"record": t.fields().map(|f| (f.name.to_owned(), describe(&f.ty))).collect::<serde_json::Map<_,_>>()})
        }
        Type::Tuple(t) => json!({"tuple": t.types().map(|t| describe(&t)).collect::<Vec<_>>()}),
        Type::Variant(t) => {
            json!({"variant": t.cases().map(|c| (c.name.to_owned(), c.ty.as_ref().map(describe).unwrap_or(Value::Null))).collect::<serde_json::Map<_,_>>()})
        }
        Type::Enum(t) => json!({"enum": t.names().collect::<Vec<_>>()}),
        Type::Flags(t) => json!({"flags": t.names().collect::<Vec<_>>()}),
        Type::Option(t) => json!({"option": describe(&t.ty())}),
        Type::Result(t) => {
            json!({"result": {"ok": t.ok().as_ref().map(describe), "err": t.err().as_ref().map(describe)}})
        }
        _ => json!(format!("{ty:?}").to_lowercase()),
    }
}

pub fn supported(ty: &Type) -> bool {
    match ty {
        Type::Own(_)
        | Type::Borrow(_)
        | Type::Future(_)
        | Type::Stream(_)
        | Type::ErrorContext
        | Type::Map(_) => false,
        Type::List(t) => supported(&t.ty()),
        Type::Record(t) => t.fields().all(|f| supported(&f.ty)),
        Type::Tuple(t) => t.types().all(|t| supported(&t)),
        Type::Variant(t) => t.cases().all(|c| c.ty.as_ref().is_none_or(supported)),
        Type::Option(t) => supported(&t.ty()),
        Type::Result(t) => {
            t.ok().as_ref().is_none_or(supported) && t.err().as_ref().is_none_or(supported)
        }
        _ => true,
    }
}

fn tagged(value: &Value) -> Result<(&str, &Value)> {
    let object = value
        .as_object()
        .context("expected a single-key tagged object")?;
    ensure!(object.len() == 1, "expected exactly one tag");
    let (tag, value) = object.iter().next().unwrap();
    Ok((tag, value))
}

fn payload(ty: Option<Type>, value: &Value) -> Result<Option<Box<Val>>> {
    match ty {
        Some(ty) => Ok(Some(Box::new(decode(&ty, value)?))),
        None => {
            ensure!(value.is_null(), "payloadless case must contain null");
            Ok(None)
        }
    }
}

pub fn decode(ty: &Type, value: &Value) -> Result<Val> {
    macro_rules! signed {
        ($variant:ident, $t:ty) => {
            Val::$variant(
                <$t>::try_from(value.as_i64().context("expected signed integer")?)
                    .context("integer out of range")?,
            )
        };
    }
    macro_rules! unsigned {
        ($variant:ident, $t:ty) => {
            Val::$variant(
                <$t>::try_from(value.as_u64().context("expected unsigned integer")?)
                    .context("integer out of range")?,
            )
        };
    }
    Ok(match ty {
        Type::Bool => Val::Bool(value.as_bool().context("expected boolean")?),
        Type::S8 => signed!(S8, i8),
        Type::S16 => signed!(S16, i16),
        Type::S32 => signed!(S32, i32),
        Type::U8 => unsigned!(U8, u8),
        Type::U16 => unsigned!(U16, u16),
        Type::U32 => unsigned!(U32, u32),
        Type::S64 => Val::S64(if let Some(s) = value.as_str() {
            s.parse().context("invalid s64 decimal string")?
        } else {
            value
                .as_i64()
                .context("expected s64 integer or decimal string")?
        }),
        Type::U64 => Val::U64(if let Some(s) = value.as_str() {
            s.parse().context("invalid u64 decimal string")?
        } else {
            value
                .as_u64()
                .context("expected u64 integer or decimal string")?
        }),
        Type::Float32 => {
            let f = value.as_f64().context("expected number")? as f32;
            ensure!(f.is_finite(), "f32 out of range");
            Val::Float32(f)
        }
        Type::Float64 => Val::Float64(value.as_f64().context("expected number")?),
        Type::String => Val::String(value.as_str().context("expected string")?.to_owned()),
        Type::Char => {
            let mut chars = value.as_str().context("expected character string")?.chars();
            let c = chars.next().context("expected one Unicode scalar")?;
            ensure!(chars.next().is_none(), "expected one Unicode scalar");
            Val::Char(c)
        }
        Type::List(t) => Val::List(
            value
                .as_array()
                .context("expected list")?
                .iter()
                .map(|v| decode(&t.ty(), v))
                .collect::<Result<_>>()?,
        ),
        Type::Record(t) => {
            let object = value.as_object().context("expected record object")?;
            ensure!(
                object.len() == t.fields().len(),
                "record fields must match exactly"
            );
            Val::Record(
                t.fields()
                    .map(|f| {
                        let v = object
                            .get(f.name)
                            .with_context(|| format!("missing field {}", f.name))?;
                        Ok((
                            f.name.to_owned(),
                            decode(&f.ty, v).with_context(|| format!("field {}", f.name))?,
                        ))
                    })
                    .collect::<Result<_>>()?,
            )
        }
        Type::Tuple(t) => {
            let array = value.as_array().context("expected tuple array")?;
            ensure!(array.len() == t.types().len(), "tuple length mismatch");
            Val::Tuple(
                t.types()
                    .zip(array)
                    .map(|(t, v)| decode(&t, v))
                    .collect::<Result<_>>()?,
            )
        }
        Type::Enum(t) => {
            let name = value.as_str().context("expected enum string")?;
            ensure!(t.names().any(|n| n == name), "unknown enum case {name}");
            Val::Enum(name.to_owned())
        }
        Type::Flags(t) => {
            let mut names = Vec::new();
            for v in value.as_array().context("expected flags array")? {
                let name = v.as_str().context("expected flag string")?;
                ensure!(t.names().any(|n| n == name), "unknown flag {name}");
                ensure!(!names.iter().any(|n| n == name), "duplicate flag {name}");
                names.push(name.to_owned());
            }
            Val::Flags(names)
        }
        Type::Variant(t) => {
            let (tag, v) = tagged(value)?;
            let case = t
                .cases()
                .find(|c| c.name == tag)
                .context("unknown variant case")?;
            Val::Variant(tag.to_owned(), payload(case.ty, v)?)
        }
        Type::Option(t) => {
            let (tag, v) = tagged(value)?;
            Val::Option(match tag {
                "none" => {
                    ensure!(v.is_null(), "none must contain null");
                    None
                }
                "some" => Some(Box::new(decode(&t.ty(), v)?)),
                _ => bail!("expected some or none tag"),
            })
        }
        Type::Result(t) => {
            let (tag, v) = tagged(value)?;
            Val::Result(match tag {
                "ok" => Ok(payload(t.ok(), v)?),
                "err" => Err(payload(t.err(), v)?),
                _ => bail!("expected ok or err tag"),
            })
        }
        _ => bail!("unsupported v0.1 value type: {ty:?}"),
    })
}

pub fn encode(value: &Val) -> Result<Value> {
    fn optional(v: &Option<Box<Val>>) -> Result<Value> {
        v.as_deref()
            .map(encode)
            .transpose()
            .map(|v| v.unwrap_or(Value::Null))
    }
    Ok(match value {
        Val::Bool(v) => json!(v),
        Val::S8(v) => json!(v),
        Val::S16(v) => json!(v),
        Val::S32(v) => json!(v),
        Val::U8(v) => json!(v),
        Val::U16(v) => json!(v),
        Val::U32(v) => json!(v),
        Val::S64(v) => json!(v.to_string()),
        Val::U64(v) => json!(v.to_string()),
        Val::Float32(v) => {
            ensure!(
                v.is_finite(),
                "non-finite float cannot be represented in JSON"
            );
            json!(v)
        }
        Val::Float64(v) => {
            ensure!(
                v.is_finite(),
                "non-finite float cannot be represented in JSON"
            );
            json!(v)
        }
        Val::Char(v) => json!(v.to_string()),
        Val::String(v) | Val::Enum(v) => json!(v),
        Val::List(v) | Val::Tuple(v) => Value::Array(v.iter().map(encode).collect::<Result<_>>()?),
        Val::Record(v) => Value::Object(
            v.iter()
                .map(|(k, v)| Ok((k.clone(), encode(v)?)))
                .collect::<Result<_>>()?,
        ),
        Val::Flags(v) => json!(v),
        Val::Variant(tag, v) => json!({tag: optional(v)?}),
        Val::Option(None) => json!({"none": null}),
        Val::Option(Some(v)) => json!({"some": encode(v)?}),
        Val::Result(Ok(v)) => json!({"ok": optional(v)?}),
        Val::Result(Err(v)) => json!({"err": optional(v)?}),
        _ => bail!("unsupported v0.1 result value"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use wasmtime::{
        Engine,
        component::{Component, types::ComponentItem},
    };

    fn ty(definition: &str) -> Type {
        let engine = Engine::default();
        let component = Component::new(
            &engine,
            format!(
                r#"(component
            (type $definition {definition})
            (import "value-type" (type $t (eq $definition)))
            (import "test" (func (param "value" $t)))
        )"#
            ),
        )
        .unwrap();
        let ComponentItem::ComponentFunc(f) = component
            .component_type()
            .imports(&engine)
            .find(|(name, _)| *name == "test")
            .unwrap()
            .1
        else {
            panic!("function expected")
        };
        f.params().next().unwrap().1
    }

    #[test]
    fn round_trips_composite_values_losslessly() {
        for (definition, value) in [
            ("(option (option u32))", json!({"some": {"none": null}})),
            ("(option (option u32))", json!({"none": null})),
            ("(result u32 (error string))", json!({"err": "failed"})),
            ("(result)", json!({"ok": null})),
            (
                "(variant (case \"empty\") (case \"value\" u32))",
                json!({"value": 7}),
            ),
            (
                "(variant (case \"empty\") (case \"value\" u32))",
                json!({"empty": null}),
            ),
            ("(tuple u32 string)", json!([7, "hello"])),
            ("(list u8)", json!([0, 128, 255])),
            (
                "(record (field \"a\" u32) (field \"b\" string))",
                json!({"a": 3, "b": "x"}),
            ),
            ("(enum \"red\" \"green\")", json!("green")),
            ("(flags \"read\" \"write\")", json!(["read", "write"])),
        ] {
            let ty = ty(definition);
            assert!(supported(&ty));
            assert_eq!(
                encode(&decode(&ty, &value).unwrap()).unwrap(),
                value,
                "{definition}"
            );
        }
    }

    #[test]
    fn rejects_invalid_tags_fields_flags_and_ranges() {
        for (definition, value) in [
            ("(option u32)", json!({"none": 1})),
            ("(option u32)", json!({"some": 1, "none": null})),
            ("(result)", json!({"ok": 1})),
            ("(variant (case \"empty\"))", json!({"unknown": null})),
            ("(record (field \"a\" u32))", json!({"b": 1})),
            ("(flags \"read\")", json!(["read", "read"])),
            ("(enum \"red\")", json!("blue")),
            ("(tuple u32)", json!([])),
        ] {
            assert!(decode(&ty(definition), &value).is_err(), "{definition}");
        }
        assert!(decode(&Type::U8, &json!(256)).is_err());
        assert!(decode(&Type::Char, &json!("ab")).is_err());
        assert!(decode(&Type::Float32, &json!(1e300)).is_err());
        assert!(encode(&Val::Float64(f64::NAN)).is_err());
    }

    #[test]
    fn preserves_full_width_integers() {
        let value = json!(u64::MAX.to_string());
        assert_eq!(encode(&decode(&Type::U64, &value).unwrap()).unwrap(), value);
        let value = json!(i64::MIN.to_string());
        assert_eq!(encode(&decode(&Type::S64, &value).unwrap()).unwrap(), value);
    }
}
