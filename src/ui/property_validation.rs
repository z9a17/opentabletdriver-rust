//! CLR scalar validation for the managed Filters editor. No plugin code runs here.
use crate::dotnet::PropertyMetadata;
use serde_json::{Number, Value};

/// Parse an editor value without rounding integer settings through floating point.
/// Explicit JSON null retains the bridge's attribute/constructor default semantics.
pub(super) fn parse(text: &str, metadata: &PropertyMetadata) -> Result<Value, String> {
    if !metadata.writable {
        return Err("This property is read-only.".into());
    }
    // Strings are literal editor text, including "null". The separate default
    // action writes the null sentinel without making string values ambiguous.
    if metadata.property_type == "System.String" {
        return Ok(Value::String(text.to_owned()));
    }
    let trimmed = text.trim();
    if trimmed == "null" {
        return Ok(Value::Null);
    }
    if metadata.enum_underlying_type.is_some() || !metadata.enum_choices.is_empty() {
        return parse_enum(trimmed, metadata);
    }
    if let Some(result) = parse_integer(trimmed, &metadata.property_type) {
        return result;
    }
    match metadata.property_type.as_str() {
        "System.Boolean" => {
            if trimmed.eq_ignore_ascii_case("true") {
                Ok(Value::Bool(true))
            } else if trimmed.eq_ignore_ascii_case("false") {
                Ok(Value::Bool(false))
            } else {
                Err("Enter true or false.".into())
            }
        }
        "System.Single" | "System.Double" => {
            let value = trimmed
                .parse::<f64>()
                .ok()
                .filter(|value| value.is_finite())
                .filter(|value| {
                    metadata.property_type != "System.Single"
                        || value.abs() <= f64::from(f32::MAX)
                })
                .and_then(Number::from_f64)
                .map(Value::Number);
            value.ok_or_else(|| {
                format!("Enter a finite number within the {} range.", metadata.property_type)
            })
        }
        "System.Decimal" => Err(
            "Decimal precision is not supported by this field. Use the filter JSON editor; a quoted decimal preserves its digits for .NET conversion.".into(),
        ),
        // Complex/custom types retain the explicit JSON editor. Parsing JSON is
        // not validation of a plugin-defined CLR type or its converters.
        _ => serde_json::from_str(trimmed).map_err(|error| {
            format!("Enter valid JSON for {}: {error}", metadata.property_type)
        }),
    }
}

fn parse_integer(text: &str, property_type: &str) -> Option<Result<Value, String>> {
    let signed = match property_type {
        "System.SByte" => Some((i64::from(i8::MIN), i64::from(i8::MAX))),
        "System.Int16" => Some((i64::from(i16::MIN), i64::from(i16::MAX))),
        "System.Int32" => Some((i64::from(i32::MIN), i64::from(i32::MAX))),
        "System.Int64" => Some((i64::MIN, i64::MAX)),
        _ => None,
    };
    if let Some((minimum, maximum)) = signed {
        return Some(
            text.parse::<i64>()
                .ok()
                .filter(|value| (minimum..=maximum).contains(value))
                .map(Value::from)
                .ok_or_else(|| format!("Enter a whole number from {minimum} to {maximum}.")),
        );
    }
    let maximum = match property_type {
        "System.Byte" => u64::from(u8::MAX),
        "System.UInt16" => u64::from(u16::MAX),
        "System.UInt32" => u64::from(u32::MAX),
        "System.UInt64" => u64::MAX,
        _ => return None,
    };
    Some(
        text.parse::<u64>()
            .ok()
            .filter(|value| *value <= maximum)
            .map(Value::from)
            .ok_or_else(|| format!("Enter a whole number from 0 to {maximum}.")),
    )
}

fn parse_enum(text: &str, metadata: &PropertyMetadata) -> Result<Value, String> {
    let decoded;
    let text = if text.starts_with('"') {
        decoded = serde_json::from_str::<String>(text)
            .map_err(|_| "Enter an enum name, integer, or JSON-quoted name.".to_owned())?;
        decoded.trim()
    } else {
        text
    };
    // Keep named selections as names. This also avoids precision loss for enums
    // with UInt64 storage and lets the bridge handle the actual enum conversion.
    let mut names = Vec::new();
    for name in text.split(',') {
        if let Some(choice) = metadata
            .enum_choices
            .iter()
            .find(|choice| choice.name.eq_ignore_ascii_case(name.trim()))
        {
            names.push(choice.name.as_str());
        } else {
            names.clear();
            break;
        }
    }
    if !names.is_empty() {
        if names.len() > 1 && !metadata.enum_flags {
            return Err("Choose one enum name; this property is not a flags enum.".into());
        }
        return Ok(Value::String(names.join(", ")));
    }
    if let Some(underlying_type) = &metadata.enum_underlying_type
        && let Some(result) = parse_integer(text, underlying_type)
    {
        return result
            .map_err(|error| format!("Enter a declared enum name or an integer. {error}"));
    }
    Err("Enter a declared enum name. Numeric values require underlying enum type metadata.".into())
}
