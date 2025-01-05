mod de;
mod error;
mod json_path;
mod ser;

use std::rc::Rc;

pub use crate::json::de::from_str;
use crate::json::json_path::{json_path, PathElement};
pub use crate::json::ser::to_string;
use crate::types::{OwnedValue, Text, TextSubtype};
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug, PartialEq, Clone)]
#[serde(untagged)]
pub enum Val {
    Null,
    Bool(bool),
    Integer(i64),
    Float(f64),
    String(String),
    Array(Vec<Val>),
    Object(IndexMap<String, Val>),
}

pub fn get_json(json_value: &OwnedValue) -> crate::Result<OwnedValue> {
    match json_value {
        OwnedValue::Text(ref t) => {
            // optimization: once we know the subtype is a valid JSON, we do not have
            // to go through parsing JSON and serializing it back to string
            if t.subtype == TextSubtype::Json {
                return Ok(json_value.to_owned());
            }

            let json_val = get_json_value(json_value)?;
            let json = crate::json::to_string(&json_val).unwrap();

            Ok(OwnedValue::Text(Text::json(Rc::new(json))))
        }
        OwnedValue::Blob(b) => {
            // TODO: use get_json_value after we implement a single Struct
            //   to represent both JSON and JSONB
            if let Ok(json) = jsonb::from_slice(b) {
                Ok(OwnedValue::Text(Text::json(Rc::new(json.to_string()))))
            } else {
                crate::bail_parse_error!("malformed JSON");
            }
        }
        OwnedValue::Null => Ok(OwnedValue::Null),
        _ => {
            let json_val = get_json_value(json_value)?;
            let json = crate::json::to_string(&json_val).unwrap();

            Ok(OwnedValue::Text(Text::json(Rc::new(json))))
        }
    }
}

fn get_json_value(json_value: &OwnedValue) -> crate::Result<Val> {
    match json_value {
        OwnedValue::Text(ref t) => match crate::json::from_str::<Val>(&t.value) {
            Ok(json) => Ok(json),
            Err(_) => {
                crate::bail_parse_error!("malformed JSON")
            }
        },
        OwnedValue::Blob(b) => {
            if let Ok(_json) = jsonb::from_slice(b) {
                todo!("jsonb to json conversion");
            } else {
                crate::bail_parse_error!("malformed JSON");
            }
        }
        OwnedValue::Null => Ok(Val::Null),
        OwnedValue::Float(f) => Ok(Val::Float(*f)),
        OwnedValue::Integer(i) => Ok(Val::Integer(*i)),
        _ => Ok(Val::String(json_value.to_string())),
    }
}

pub fn json_array(values: &[OwnedValue]) -> crate::Result<OwnedValue> {
    let mut s = String::new();
    s.push('[');

    for (idx, value) in values.iter().enumerate() {
        match value {
            OwnedValue::Blob(_) => crate::bail_constraint_error!("JSON cannot hold BLOB values"),
            OwnedValue::Text(t) => {
                if t.subtype == TextSubtype::Json {
                    s.push_str(&t.value);
                } else {
                    match crate::json::to_string(&t.value.as_ref().to_string()) {
                        Ok(json) => s.push_str(&json),
                        Err(_) => crate::bail_parse_error!("malformed JSON"),
                    }
                }
            }
            OwnedValue::Integer(i) => match crate::json::to_string(&i) {
                Ok(json) => s.push_str(&json),
                Err(_) => crate::bail_parse_error!("malformed JSON"),
            },
            OwnedValue::Float(f) => match crate::json::to_string(&f) {
                Ok(json) => s.push_str(&json),
                Err(_) => crate::bail_parse_error!("malformed JSON"),
            },
            OwnedValue::Null => s.push_str("null"),
            _ => unreachable!(),
        }

        if idx < values.len() - 1 {
            s.push(',');
        }
    }

    s.push(']');
    Ok(OwnedValue::Text(Text::json(Rc::new(s))))
}

pub fn json_array_length(
    json_value: &OwnedValue,
    json_path: Option<&OwnedValue>,
) -> crate::Result<OwnedValue> {
    let path = match json_path {
        Some(OwnedValue::Text(t)) => Some(t.value.to_string()),
        Some(OwnedValue::Integer(i)) => Some(i.to_string()),
        Some(OwnedValue::Float(f)) => Some(f.to_string()),
        _ => None::<String>,
    };

    let json = get_json_value(json_value)?;

    let arr_val = if let Some(path) = path {
        &json_extract_single(&json, path.as_str())?
    } else {
        &json
    };

    match arr_val {
        Val::Array(val) => Ok(OwnedValue::Integer(val.len() as i64)),
        Val::Null => Ok(OwnedValue::Null),
        _ => Ok(OwnedValue::Integer(0)),
    }
}

pub fn json_extract(value: &OwnedValue, paths: &[OwnedValue]) -> crate::Result<OwnedValue> {
    if let OwnedValue::Null = value {
        return Ok(OwnedValue::Null);
    }

    if paths.is_empty() {
        return Ok(OwnedValue::Null);
    }

    let json = get_json_value(value)?;
    let mut result = "".to_string();

    if paths.len() > 1 {
        result.push('[');
    }

    for path in paths {
        match path {
            OwnedValue::Text(p) => {
                let extracted = json_extract_single(&json, p.value.as_ref())?;

                if paths.len() == 1 && extracted == Val::Null {
                    return Ok(OwnedValue::Null);
                }

                result.push_str(&crate::json::to_string(&extracted).unwrap());
                if paths.len() > 1 {
                    result.push(',');
                }
            }
            OwnedValue::Null => return Ok(OwnedValue::Null),
            _ => crate::bail_constraint_error!("JSON path error near: {:?}", path.to_string()),
        }
    }

    if paths.len() > 1 {
        result.pop(); // remove the final comma
        result.push(']');
    }

    Ok(OwnedValue::Text(Text::json(Rc::new(result))))
}

fn json_extract_single(json: &Val, path: &str) -> crate::Result<Val> {
    let json_path = json_path(path)?;

    let mut current_element = &Val::Null;

    for element in json_path.elements.iter() {
        match element {
            PathElement::Root() => {
                current_element = json;
            }
            PathElement::Key(key) => {
                let key = key.as_str();

                match current_element {
                    Val::Object(map) => {
                        if let Some(value) = map.get(key) {
                            current_element = value;
                        } else {
                            return Ok(Val::Null);
                        }
                    }
                    _ => {
                        return Ok(Val::Null);
                    }
                }
            }
            PathElement::ArrayLocator(idx) => match current_element {
                Val::Array(array) => {
                    let mut idx = *idx;

                    if idx < 0 {
                        idx += array.len() as i32;
                    }

                    if idx < array.len() as i32 {
                        current_element = &array[idx as usize];
                    } else {
                        return Ok(Val::Null);
                    }
                }
                _ => {
                    return Ok(Val::Null);
                }
            },
        }
    }
    Ok(current_element.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::OwnedValue;

    #[test]
    fn test_get_json_valid_json5() {
        let input = OwnedValue::build_text(Rc::new("{ key: 'value' }".to_string()));
        let result = get_json(&input).unwrap();
        if let OwnedValue::Text(result_str) = result {
            assert!(result_str.value.contains("\"key\":\"value\""));
            assert_eq!(result_str.subtype, TextSubtype::Json);
        } else {
            panic!("Expected OwnedValue::Text");
        }
    }

    #[test]
    fn test_get_json_valid_json5_double_single_quotes() {
        let input = OwnedValue::build_text(Rc::new("{ key: ''value'' }".to_string()));
        let result = get_json(&input).unwrap();
        if let OwnedValue::Text(result_str) = result {
            assert!(result_str.value.contains("\"key\":\"value\""));
            assert_eq!(result_str.subtype, TextSubtype::Json);
        } else {
            panic!("Expected OwnedValue::Text");
        }
    }

    #[test]
    fn test_get_json_valid_json5_infinity() {
        let input = OwnedValue::build_text(Rc::new("{ \"key\": Infinity }".to_string()));
        let result = get_json(&input).unwrap();
        if let OwnedValue::Text(result_str) = result {
            assert!(result_str.value.contains("{\"key\":9e999}"));
            assert_eq!(result_str.subtype, TextSubtype::Json);
        } else {
            panic!("Expected OwnedValue::Text");
        }
    }

    #[test]
    fn test_get_json_valid_json5_negative_infinity() {
        let input = OwnedValue::build_text(Rc::new("{ \"key\": -Infinity }".to_string()));
        let result = get_json(&input).unwrap();
        if let OwnedValue::Text(result_str) = result {
            assert!(result_str.value.contains("{\"key\":-9e999}"));
            assert_eq!(result_str.subtype, TextSubtype::Json);
        } else {
            panic!("Expected OwnedValue::Text");
        }
    }

    #[test]
    fn test_get_json_valid_json5_nan() {
        let input = OwnedValue::build_text(Rc::new("{ \"key\": NaN }".to_string()));
        let result = get_json(&input).unwrap();
        if let OwnedValue::Text(result_str) = result {
            assert!(result_str.value.contains("{\"key\":null}"));
            assert_eq!(result_str.subtype, TextSubtype::Json);
        } else {
            panic!("Expected OwnedValue::Text");
        }
    }

    #[test]
    fn test_get_json_invalid_json5() {
        let input = OwnedValue::build_text(Rc::new("{ key: value }".to_string()));
        let result = get_json(&input);
        match result {
            Ok(_) => panic!("Expected error for malformed JSON"),
            Err(e) => assert!(e.to_string().contains("malformed JSON")),
        }
    }

    #[test]
    fn test_get_json_valid_jsonb() {
        let input = OwnedValue::build_text(Rc::new("{\"key\":\"value\"}".to_string()));
        let result = get_json(&input).unwrap();
        if let OwnedValue::Text(result_str) = result {
            assert!(result_str.value.contains("\"key\":\"value\""));
            assert_eq!(result_str.subtype, TextSubtype::Json);
        } else {
            panic!("Expected OwnedValue::Text");
        }
    }

    #[test]
    fn test_get_json_invalid_jsonb() {
        let input = OwnedValue::build_text(Rc::new("{key:\"value\"".to_string()));
        let result = get_json(&input);
        match result {
            Ok(_) => panic!("Expected error for malformed JSON"),
            Err(e) => assert!(e.to_string().contains("malformed JSON")),
        }
    }

    #[test]
    fn test_get_json_blob_valid_jsonb() {
        let binary_json = b"\x40\0\0\x01\x10\0\0\x03\x10\0\0\x03\x61\x73\x64\x61\x64\x66".to_vec();
        let input = OwnedValue::Blob(Rc::new(binary_json));
        let result = get_json(&input).unwrap();
        if let OwnedValue::Text(result_str) = result {
            assert!(result_str.value.contains("\"asd\":\"adf\""));
            assert_eq!(result_str.subtype, TextSubtype::Json);
        } else {
            panic!("Expected OwnedValue::Text");
        }
    }

    #[test]
    fn test_get_json_blob_invalid_jsonb() {
        let binary_json: Vec<u8> = vec![0xA2, 0x62, 0x6B, 0x31, 0x62, 0x76]; // Incomplete binary JSON
        let input = OwnedValue::Blob(Rc::new(binary_json));
        let result = get_json(&input);
        match result {
            Ok(_) => panic!("Expected error for malformed JSON"),
            Err(e) => assert!(e.to_string().contains("malformed JSON")),
        }
    }

    #[test]
    fn test_get_json_non_text() {
        let input = OwnedValue::Null;
        let result = get_json(&input).unwrap();
        if let OwnedValue::Null = result {
            // Test passed
        } else {
            panic!("Expected OwnedValue::Null");
        }
    }

    #[test]
    fn test_json_array_simple() {
        let text = OwnedValue::build_text(Rc::new("value1".to_string()));
        let json = OwnedValue::Text(Text::json(Rc::new("\"value2\"".to_string())));
        let input = vec![text, json, OwnedValue::Integer(1), OwnedValue::Float(1.1)];

        let result = json_array(&input).unwrap();
        if let OwnedValue::Text(res) = result {
            assert_eq!(res.value.as_ref(), "[\"value1\",\"value2\",1,1.1]");
            assert_eq!(res.subtype, TextSubtype::Json);
        } else {
            panic!("Expected OwnedValue::Text");
        }
    }

    #[test]
    fn test_json_array_empty() {
        let input = vec![];

        let result = json_array(&input).unwrap();
        if let OwnedValue::Text(res) = result {
            assert_eq!(res.value.as_ref(), "[]");
            assert_eq!(res.subtype, TextSubtype::Json);
        } else {
            panic!("Expected OwnedValue::Text");
        }
    }

    #[test]
    fn test_json_array_blob_invalid() {
        let blob = OwnedValue::Blob(Rc::new("1".as_bytes().to_vec()));

        let input = vec![blob];

        let result = json_array(&input);

        match result {
            Ok(_) => panic!("Expected error for blob input"),
            Err(e) => assert!(e.to_string().contains("JSON cannot hold BLOB values")),
        }
    }

    #[test]
    fn test_json_array_length() {
        let input = OwnedValue::build_text(Rc::new("[1,2,3,4]".to_string()));
        let result = json_array_length(&input, None).unwrap();
        if let OwnedValue::Integer(res) = result {
            assert_eq!(res, 4);
        } else {
            panic!("Expected OwnedValue::Integer");
        }
    }

    #[test]
    fn test_json_array_length_empty() {
        let input = OwnedValue::build_text(Rc::new("[]".to_string()));
        let result = json_array_length(&input, None).unwrap();
        if let OwnedValue::Integer(res) = result {
            assert_eq!(res, 0);
        } else {
            panic!("Expected OwnedValue::Integer");
        }
    }

    #[test]
    fn test_json_array_length_root() {
        let input = OwnedValue::build_text(Rc::new("[1,2,3,4]".to_string()));
        let result = json_array_length(
            &input,
            Some(&OwnedValue::build_text(Rc::new("$".to_string()))),
        )
        .unwrap();
        if let OwnedValue::Integer(res) = result {
            assert_eq!(res, 4);
        } else {
            panic!("Expected OwnedValue::Integer");
        }
    }

    #[test]
    fn test_json_array_length_not_array() {
        let input = OwnedValue::build_text(Rc::new("{one: [1,2,3,4]}".to_string()));
        let result = json_array_length(&input, None).unwrap();
        if let OwnedValue::Integer(res) = result {
            assert_eq!(res, 0);
        } else {
            panic!("Expected OwnedValue::Integer");
        }
    }

    #[test]
    fn test_json_array_length_via_prop() {
        let input = OwnedValue::build_text(Rc::new("{one: [1,2,3,4]}".to_string()));
        let result = json_array_length(
            &input,
            Some(&OwnedValue::build_text(Rc::new("$.one".to_string()))),
        )
        .unwrap();
        if let OwnedValue::Integer(res) = result {
            assert_eq!(res, 4);
        } else {
            panic!("Expected OwnedValue::Integer");
        }
    }

    #[test]
    fn test_json_array_length_via_index() {
        let input = OwnedValue::build_text(Rc::new("[[1,2,3,4]]".to_string()));
        let result = json_array_length(
            &input,
            Some(&OwnedValue::build_text(Rc::new("$[0]".to_string()))),
        )
        .unwrap();
        if let OwnedValue::Integer(res) = result {
            assert_eq!(res, 4);
        } else {
            panic!("Expected OwnedValue::Integer");
        }
    }

    #[test]
    fn test_json_array_length_via_index_not_array() {
        let input = OwnedValue::build_text(Rc::new("[1,2,3,4]".to_string()));
        let result = json_array_length(
            &input,
            Some(&OwnedValue::build_text(Rc::new("$[2]".to_string()))),
        )
        .unwrap();
        if let OwnedValue::Integer(res) = result {
            assert_eq!(res, 0);
        } else {
            panic!("Expected OwnedValue::Integer");
        }
    }

    #[test]
    fn test_json_array_length_via_index_bad_prop() {
        let input = OwnedValue::build_text(Rc::new("{one: [1,2,3,4]}".to_string()));
        let result = json_array_length(
            &input,
            Some(&OwnedValue::build_text(Rc::new("$.two".to_string()))),
        )
        .unwrap();
        assert_eq!(OwnedValue::Null, result);
    }

    #[test]
    fn test_json_array_length_simple_json_subtype() {
        let input = OwnedValue::build_text(Rc::new("[1,2,3]".to_string()));
        let wrapped = get_json(&input).unwrap();
        let result = json_array_length(&wrapped, None).unwrap();

        if let OwnedValue::Integer(res) = result {
            assert_eq!(res, 3);
        } else {
            panic!("Expected OwnedValue::Integer");
        }
    }

    #[test]
    fn test_json_extract_missing_path() {
        let result = json_extract(
            &OwnedValue::build_text(Rc::new("{\"a\":2}".to_string())),
            &[OwnedValue::build_text(Rc::new("$.x".to_string()))],
        );

        match result {
            Ok(OwnedValue::Null) => (),
            _ => panic!("Expected null result, got: {:?}", result),
        }
    }
    #[test]
    fn test_json_extract_null_path() {
        let result = json_extract(
            &OwnedValue::build_text(Rc::new("{\"a\":2}".to_string())),
            &[OwnedValue::Null],
        );

        match result {
            Ok(OwnedValue::Null) => (),
            _ => panic!("Expected null result, got: {:?}", result),
        }
    }

    #[test]
    fn test_json_path_invalid() {
        let result = json_extract(
            &OwnedValue::build_text(Rc::new("{\"a\":2}".to_string())),
            &[OwnedValue::Float(1.1)],
        );

        match result {
            Ok(_) => panic!("expected error"),
            Err(e) => assert!(e.to_string().contains("JSON path error")),
        }
    }
}
