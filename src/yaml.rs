use crate::parser::*;
use crate::scanner::{Marker, ScanError, TScalarStyle, TokenType};

use hashlink::LinkedHashMap;
use std::collections::BTreeMap;
use std::f64;
use std::i64;
use std::mem;
use std::ops::Index;
use std::string;
use std::vec;

/// A YAML node is stored as this `Yaml` enumeration, which provides an easy way to
/// access your YAML document.
///
/// # Examples
///
/// ```
/// # extern crate yaml_rust_davvid as yaml_rust;
/// use yaml_rust::Yaml;
/// let foo = Yaml::from_str("-123"); // convert the string to the appropriate YAML type
/// assert_eq!(foo.as_i64().unwrap(), -123);
///
/// // iterate over an Array
/// let vec = Yaml::Array(vec![Yaml::Integer(1), Yaml::Integer(2)]);
/// for v in vec.as_vec().unwrap() {
///     assert!(v.as_i64().is_some());
/// }
/// ```
#[derive(Clone, PartialEq, PartialOrd, Debug, Eq, Ord, Hash)]
pub enum Yaml {
    /// Float types are stored as String and parsed on demand.
    /// Note that f64 does NOT implement Eq trait and can NOT be stored in BTreeMap.
    Real(string::String),
    /// YAML int is stored as i64.
    Integer(i64),
    /// YAML scalar.
    String(string::String),
    /// YAML bool, e.g. `true` or `false`.
    Boolean(bool),
    /// YAML array, can be accessed as a `Vec`.
    Array(self::Array),
    /// YAML hash, can be accessed as a `LinkedHashMap`.
    ///
    /// Insertion order will match the order of insertion into the map.
    Hash(self::Hash),
    /// Alias, not fully supported yet.
    Alias(usize),
    /// YAML null, e.g. `null` or `~`.
    Null,
    /// Accessing a nonexistent node via the Index trait returns `BadValue`. This
    /// simplifies error handling in the calling code. Invalid type conversion also
    /// returns `BadValue`.
    BadValue,
}

pub type Array = Vec<Yaml>;
pub type Hash = LinkedHashMap<Yaml, Yaml>;

// parse f64 as Core schema
// See: https://github.com/chyh1990/yaml-rust/issues/51
fn parse_f64(v: &str) -> Option<f64> {
    match v {
        ".inf" | ".Inf" | ".INF" | "+.inf" | "+.Inf" | "+.INF" => Some(f64::INFINITY),
        "-.inf" | "-.Inf" | "-.INF" => Some(f64::NEG_INFINITY),
        ".nan" | "NaN" | ".NAN" => Some(f64::NAN),
        _ => v.parse::<f64>().ok(),
    }
}

/// A `YamlScalarParser` is a parser that change the parsing of a yaml scalar value
/// like a tag
///
/// # Examples
///
/// ```
/// # extern crate yaml_rust_davvid as yaml_rust;
///use yaml_rust::yaml;
///use yaml_rust::scanner;
///
///struct HelloTagParser;
///
///impl yaml::YamlScalarParser for HelloTagParser {
///    fn parse_scalar(&self, tag: &scanner::TokenType, value: &str) -> Option<yaml::Yaml> {
///        if let scanner::TokenType::Tag(ref handle, ref suffix) = *tag {
///            if *handle == "!" && *suffix == "hello" {
///                return Some(yaml::Yaml::String("Hello ".to_string() + value))
///            }
///        }
///        None
///    }
///}
/// ```
pub trait YamlScalarParser {
    fn parse_scalar(&self, tag: &TokenType, value: &str) -> Option<Yaml>;
}

#[derive(Default)]
pub struct YamlLoader<'a> {
    docs: Vec<Yaml>,
    // states
    // (current node, anchor_id) tuple
    doc_stack: Vec<(Yaml, usize)>,
    key_stack: Vec<Yaml>,
    anchor_map: BTreeMap<usize, Yaml>,
    scalar_parser: Vec<&'a dyn YamlScalarParser>,
}

impl<'a> MarkedEventReceiver for YamlLoader<'a> {
    fn on_event(&mut self, ev: Event, _: Marker) {
        // println!("EV {:?}", ev);
        match ev {
            Event::DocumentStart => {
                // do nothing
            }
            Event::DocumentEnd => {
                match self.doc_stack.len() {
                    // empty document
                    0 => self.docs.push(Yaml::BadValue),
                    1 => self.docs.push(self.doc_stack.pop().unwrap().0),
                    _ => unreachable!(),
                }
            }
            Event::SequenceStart(aid) => {
                self.doc_stack.push((Yaml::Array(Vec::new()), aid));
            }
            Event::SequenceEnd => {
                let node = self.doc_stack.pop().unwrap();
                self.insert_new_node(node);
            }
            Event::MappingStart(aid) => {
                self.doc_stack.push((Yaml::Hash(Hash::new()), aid));
                self.key_stack.push(Yaml::BadValue);
            }
            Event::MappingEnd => {
                self.key_stack.pop().unwrap();
                let node = self.doc_stack.pop().unwrap();
                self.insert_new_node(node);
            }
            Event::Scalar(v, style, aid, tag) => {
                if let Some(ref tag) = tag {
                    let mut yaml = None;
                    for parser in &self.scalar_parser {
                        yaml = parser.parse_scalar(tag, &v);
                    }
                    if let Some(yaml) = yaml {
                        self.insert_new_node((yaml, aid));
                        return;
                    }
                }

                let node = if style != TScalarStyle::Plain {
                    Yaml::String(v)
                } else if let Some(TokenType::Tag(ref handle, ref suffix)) = tag {
                    // XXX tag:yaml.org,2002:
                    if handle == "!!" {
                        match suffix.as_ref() {
                            "bool" => {
                                // "true" or "false"
                                match v.parse::<bool>() {
                                    Err(_) => Yaml::BadValue,
                                    Ok(v) => Yaml::Boolean(v),
                                }
                            }
                            "int" => match v.parse::<i64>() {
                                Err(_) => Yaml::BadValue,
                                Ok(v) => Yaml::Integer(v),
                            },
                            "float" => match parse_f64(&v) {
                                Some(_) => Yaml::Real(v),
                                None => Yaml::BadValue,
                            },
                            "null" => match v.as_ref() {
                                "~" | "null" => Yaml::Null,
                                _ => Yaml::BadValue,
                            },
                            _ => Yaml::String(v),
                        }
                    } else {
                        Yaml::String(v)
                    }
                } else {
                    // Datatype is not specified, or unrecognized
                    Yaml::from_str(&v)
                };

                self.insert_new_node((node, aid));
            }
            Event::Alias(id) => {
                let n = match self.anchor_map.get(&id) {
                    Some(v) => v.clone(),
                    None => Yaml::BadValue,
                };
                self.insert_new_node((n, 0));
            }
            _ => { /* ignore */ }
        }
        // println!("DOC {:?}", self.doc_stack);
    }
}

#[derive(Debug)]
pub enum LoadError {
    IO(std::io::Error),
    Scan(ScanError),
    Decode(std::borrow::Cow<'static, str>),
}

impl From<std::io::Error> for LoadError {
    fn from(error: std::io::Error) -> Self {
        LoadError::IO(error)
    }
}

impl<'a> YamlLoader<'a> {
    fn insert_new_node(&mut self, node: (Yaml, usize)) {
        // valid anchor id starts from 1
        if node.1 > 0 {
            self.anchor_map.insert(node.1, node.0.clone());
        }
        if self.doc_stack.is_empty() {
            self.doc_stack.push(node);
        } else {
            let parent = self.doc_stack.last_mut().unwrap();
            match *parent {
                (Yaml::Array(ref mut v), _) => v.push(node.0),
                (Yaml::Hash(ref mut h), _) => {
                    let cur_key = self.key_stack.last_mut().unwrap();
                    // current node is a key
                    if cur_key.is_badvalue() {
                        *cur_key = node.0;
                    // current node is a value
                    } else {
                        let mut newkey = Yaml::BadValue;
                        mem::swap(&mut newkey, cur_key);
                        h.insert(newkey, node.0);
                    }
                }
                _ => unreachable!(),
            }
        }
    }

    pub fn register_scalar_parser(&mut self, parser: &'a dyn YamlScalarParser) {
        self.scalar_parser.push(parser);
    }

    pub fn load_from_str(source: &str) -> Result<Vec<Yaml>, ScanError> {
        YamlLoader::new().parse_from_str(source)
    }

    pub fn new() -> YamlLoader<'a> {
        YamlLoader {
            docs: Vec::new(),
            doc_stack: Vec::new(),
            key_stack: Vec::new(),
            anchor_map: BTreeMap::new(),
            scalar_parser: Vec::new(),
        }
    }

    pub fn parse_from_str(mut self, source: &str) -> Result<Vec<Yaml>, ScanError> {
        let mut parser = Parser::new(source.chars());
        parser.load(&mut self, true)?;
        Ok(self.docs)
    }
}

pub struct YamlDecoder<T: std::io::Read> {
    source: T,
    trap: encoding::types::DecoderTrap,
}

impl<T: std::io::Read> YamlDecoder<T> {
    pub fn read(source: T) -> YamlDecoder<T> {
        YamlDecoder {
            source,
            trap: encoding::DecoderTrap::Strict,
        }
    }

    pub fn encoding_trap(&mut self, trap: encoding::types::DecoderTrap) -> &mut Self {
        self.trap = trap;
        self
    }

    pub fn decode(&mut self) -> Result<Vec<Yaml>, LoadError> {
        let mut buffer = Vec::new();
        self.source.read_to_end(&mut buffer)?;

        // Decodes the input buffer using either UTF-8, UTF-16LE or UTF-16BE depending on the BOM codepoint.
        // If the buffer doesn't start with a BOM codepoint, it will use a fallback encoding obtained by
        // detect_utf16_endianness.
        let (res, _) =
            encoding::types::decode(&buffer, self.trap, detect_utf16_endianness(&buffer));
        let s = res.map_err(LoadError::Decode)?;
        YamlLoader::load_from_str(&s).map_err(LoadError::Scan)
    }
}

/// The encoding crate knows how to tell apart UTF-8 from UTF-16LE and utf-16BE, when the
/// bytestream starts with BOM codepoint.
/// However, it doesn't even attempt to guess the UTF-16 endianness of the input bytestream since
/// in the general case the bytestream could start with a codepoint that uses both bytes.
///
/// The YAML-1.2 spec mandates that the first character of a YAML document is an ASCII character.
/// This allows the encoding to be deduced by the pattern of null (#x00) characters.
//
/// See spec at https://yaml.org/spec/1.2/spec.html#id2771184
fn detect_utf16_endianness(b: &[u8]) -> encoding::types::EncodingRef {
    if b.len() > 1 && (b[0] != b[1]) {
        if b[0] == 0 {
            return encoding::all::UTF_16BE;
        } else if b[1] == 0 {
            return encoding::all::UTF_16LE;
        }
    }
    encoding::all::UTF_8
}

macro_rules! define_as (
    ($name:ident, $t:ident, $yt:ident) => (
pub fn $name(&self) -> Option<$t> {
    match *self {
        Yaml::$yt(v) => Some(v),
        _ => None
    }
}
    );
);

macro_rules! define_as_ref (
    ($name:ident, $t:ty, $yt:ident) => (
pub fn $name(&self) -> Option<$t> {
    match *self {
        Yaml::$yt(ref v) => Some(v),
        _ => None
    }
}
    );
);

macro_rules! define_into (
    ($name:ident, $t:ty, $yt:ident) => (
pub fn $name(self) -> Option<$t> {
    match self {
        Yaml::$yt(v) => Some(v),
        _ => None
    }
}
    );
);

impl Yaml {
    define_as!(as_bool, bool, Boolean);
    define_as!(as_i64, i64, Integer);

    define_as_ref!(as_str, &str, String);
    define_as_ref!(as_hash, &Hash, Hash);
    define_as_ref!(as_vec, &Array, Array);

    define_into!(into_bool, bool, Boolean);
    define_into!(into_i64, i64, Integer);
    define_into!(into_string, String, String);
    define_into!(into_hash, Hash, Hash);
    define_into!(into_vec, Array, Array);

    pub fn is_null(&self) -> bool {
        matches!(*self, Yaml::Null)
    }

    pub fn is_badvalue(&self) -> bool {
        matches!(*self, Yaml::BadValue)
    }

    pub fn is_array(&self) -> bool {
        matches!(*self, Yaml::Array(_))
    }

    pub fn as_f64(&self) -> Option<f64> {
        match *self {
            Yaml::Real(ref v) => parse_f64(v),
            _ => None,
        }
    }

    pub fn into_f64(self) -> Option<f64> {
        match self {
            Yaml::Real(ref v) => parse_f64(v),
            _ => None,
        }
    }

    /// If a value is null or otherwise bad (see variants), consume it and
    /// replace it with a given value `other`. Otherwise, return self unchanged.
    ///
    /// ```
    /// # extern crate yaml_rust_davvid as yaml_rust;
    /// use yaml_rust::yaml::Yaml;
    ///
    /// assert_eq!(Yaml::BadValue.or(Yaml::Integer(3)),  Yaml::Integer(3));
    /// assert_eq!(Yaml::Integer(3).or(Yaml::BadValue),  Yaml::Integer(3));
    /// ```
    pub fn or(self, other: Self) -> Self {
        match self {
            Yaml::BadValue | Yaml::Null => other,
            this => this,
        }
    }

    /// See `or` for behavior. This performs the same operations, but with
    /// borrowed values for less linear pipelines.
    pub fn borrowed_or<'a>(&'a self, other: &'a Self) -> &'a Self {
        match self {
            Yaml::BadValue | Yaml::Null => other,
            this => this,
        }
    }
}

#[cfg_attr(feature = "cargo-clippy", allow(should_implement_trait))]
impl Yaml {
    // Not implementing FromStr because there is no possibility of Error.
    // This function falls back to Yaml::String if nothing else matches.
    pub fn from_str(v: &str) -> Yaml {
        if let Some(value) = v.strip_prefix("0x") {
            if let Ok(i) = i64::from_str_radix(value, 16) {
                return Yaml::Integer(i);
            }
        }
        if let Some(value) = v.strip_prefix("0o") {
            if let Ok(i) = i64::from_str_radix(value, 8) {
                return Yaml::Integer(i);
            }
        }
        if let Some(value) = v.strip_prefix('+') {
            if let Ok(i) = value.parse::<i64>() {
                return Yaml::Integer(i);
            }
        }
        match v {
            "~" | "null" => Yaml::Null,
            "true" => Yaml::Boolean(true),
            "false" => Yaml::Boolean(false),
            _ if v.parse::<i64>().is_ok() => Yaml::Integer(v.parse::<i64>().unwrap()),
            // try parsing as f64
            _ if parse_f64(v).is_some() => Yaml::Real(v.to_owned()),
            _ => Yaml::String(v.to_owned()),
        }
    }
}

static BAD_VALUE: Yaml = Yaml::BadValue;
impl<'a> Index<&'a str> for Yaml {
    type Output = Yaml;

    fn index(&self, idx: &'a str) -> &Yaml {
        let key = Yaml::String(idx.to_owned());
        match self.as_hash() {
            Some(h) => h.get(&key).unwrap_or(&BAD_VALUE),
            None => &BAD_VALUE,
        }
    }
}

impl Index<usize> for Yaml {
    type Output = Yaml;

    fn index(&self, idx: usize) -> &Yaml {
        if let Some(v) = self.as_vec() {
            v.get(idx).unwrap_or(&BAD_VALUE)
        } else if let Some(v) = self.as_hash() {
            let key = Yaml::Integer(idx as i64);
            v.get(&key).unwrap_or(&BAD_VALUE)
        } else {
            &BAD_VALUE
        }
    }
}

impl IntoIterator for Yaml {
    type Item = Yaml;
    type IntoIter = YamlIter;

    fn into_iter(self) -> Self::IntoIter {
        YamlIter {
            yaml: self.into_vec().unwrap_or_default().into_iter(),
        }
    }
}

pub struct YamlIter {
    yaml: vec::IntoIter<Yaml>,
}

impl Iterator for YamlIter {
    type Item = Yaml;

    fn next(&mut self) -> Option<Yaml> {
        self.yaml.next()
    }
}

#[cfg(test)]
mod test {
    use crate::scanner::*;
    use crate::yaml::*;
    use std::f64;

    #[test]
    fn test_coerce() {
        let s = "---
a: 1
b: 2.2
c: [1, 2]
";
        let out = YamlLoader::load_from_str(s).unwrap();
        let doc = &out[0];
        assert_eq!(doc["a"].as_i64().unwrap(), 1i64);
        assert_eq!(doc["b"].as_f64().unwrap(), 2.2f64);
        assert_eq!(doc["c"][1].as_i64().unwrap(), 2i64);
        assert!(doc["d"][0].is_badvalue());
    }

    #[test]
    fn test_empty_doc() {
        let s: String = "".to_owned();
        YamlLoader::load_from_str(&s).unwrap();
        let s: String = "---".to_owned();
        assert_eq!(YamlLoader::load_from_str(&s).unwrap()[0], Yaml::Null);
    }

    #[test]
    fn test_parser() {
        let s: String = "
# comment
a0 bb: val
a1:
    b1: 4
    b2: d
a2: 4 # i'm comment
a3: [1, 2, 3]
a4:
    - - a1
      - a2
    - 2
a5: 'single_quoted'
a6: \"double_quoted\"
a7: 你好
"
        .to_owned();
        let out = YamlLoader::load_from_str(&s).unwrap();
        let doc = &out[0];
        assert_eq!(doc["a7"].as_str().unwrap(), "你好");
    }

    #[test]
    fn test_multi_doc() {
        let s = "
'a scalar'
---
'a scalar'
---
'a scalar'
";
        let out = YamlLoader::load_from_str(s).unwrap();
        assert_eq!(out.len(), 3);
    }

    #[test]
    fn test_anchor() {
        let s = "
a1: &DEFAULT
    b1: 4
    b2: d
a2: *DEFAULT
";
        let out = YamlLoader::load_from_str(s).unwrap();
        let doc = &out[0];
        assert_eq!(doc["a2"]["b1"].as_i64().unwrap(), 4);
    }

    #[test]
    fn test_bad_anchor() {
        let s = "
a1: &DEFAULT
    b1: 4
    b2: *DEFAULT
";
        let out = YamlLoader::load_from_str(s).unwrap();
        let doc = &out[0];
        assert_eq!(doc["a1"]["b2"], Yaml::BadValue);
    }

    #[test]
    fn test_github_27() {
        // https://github.com/chyh1990/yaml-rust/issues/27
        let s = "&a";
        let out = YamlLoader::load_from_str(s).unwrap();
        let doc = &out[0];
        assert_eq!(doc.as_str().unwrap(), "");
    }

    #[test]
    #[allow(clippy::cognitive_complexity)]
    fn test_plain_datatype() {
        let s = "
- 'string'
- \"string\"
- string
- 123
- -321
- 1.23
- -1e4
- ~
- null
- true
- false
- !!str 0
- !!int 100
- !!float 2
- !!null ~
- !!bool true
- !!bool false
- 0xFF
# bad values
- !!int string
- !!float string
- !!bool null
- !!null val
- 0o77
- [ 0xF, 0xF ]
- +12345
- [ true, false ]
";
        let out = YamlLoader::load_from_str(s).unwrap();
        let doc = &out[0];

        assert_eq!(doc[0].as_str().unwrap(), "string");
        assert_eq!(doc[1].as_str().unwrap(), "string");
        assert_eq!(doc[2].as_str().unwrap(), "string");
        assert_eq!(doc[3].as_i64().unwrap(), 123);
        assert_eq!(doc[4].as_i64().unwrap(), -321);
        assert_eq!(doc[5].as_f64().unwrap(), 1.23);
        assert_eq!(doc[6].as_f64().unwrap(), -1e4);
        assert!(doc[7].is_null());
        assert!(doc[8].is_null());
        assert!(doc[9].as_bool().unwrap());
        assert!(!doc[10].as_bool().unwrap());
        assert_eq!(doc[11].as_str().unwrap(), "0");
        assert_eq!(doc[12].as_i64().unwrap(), 100);
        assert_eq!(doc[13].as_f64().unwrap(), 2.0);
        assert!(doc[14].is_null());
        assert!(doc[15].as_bool().unwrap());
        assert!(!doc[16].as_bool().unwrap());
        assert_eq!(doc[17].as_i64().unwrap(), 255);
        assert!(doc[18].is_badvalue());
        assert!(doc[19].is_badvalue());
        assert!(doc[20].is_badvalue());
        assert!(doc[21].is_badvalue());
        assert_eq!(doc[22].as_i64().unwrap(), 63);
        assert_eq!(doc[23][0].as_i64().unwrap(), 15);
        assert_eq!(doc[23][1].as_i64().unwrap(), 15);
        assert_eq!(doc[24].as_i64().unwrap(), 12345);
        assert!(doc[25][0].as_bool().unwrap());
        assert!(!doc[25][1].as_bool().unwrap());
    }

    #[test]
    fn test_bad_hyphen() {
        // See: https://github.com/chyh1990/yaml-rust/issues/23
        let s = "{-";
        assert!(YamlLoader::load_from_str(s).is_err());
    }

    #[test]
    fn test_issue_65() {
        // See: https://github.com/chyh1990/yaml-rust/issues/65
        let b = "\n\"ll\\\"ll\\\r\n\"ll\\\"ll\\\r\r\r\rU\r\r\rU";
        assert!(YamlLoader::load_from_str(b).is_err());
    }

    #[test]
    fn test_bad_docstart() {
        assert!(YamlLoader::load_from_str("---This used to cause an infinite loop").is_ok());
        assert_eq!(
            YamlLoader::load_from_str("----"),
            Ok(vec![Yaml::String(String::from("----"))])
        );
        assert_eq!(
            YamlLoader::load_from_str("--- #here goes a comment"),
            Ok(vec![Yaml::Null])
        );
        assert_eq!(
            YamlLoader::load_from_str("---- #here goes a comment"),
            Ok(vec![Yaml::String(String::from("----"))])
        );
    }

    #[test]
    fn test_plain_datatype_with_into_methods() {
        let s = "
- 'string'
- \"string\"
- string
- 123
- -321
- 1.23
- -1e4
- true
- false
- !!str 0
- !!int 100
- !!float 2
- !!bool true
- !!bool false
- 0xFF
- 0o77
- +12345
- -.INF
- .NAN
- !!float .INF
";
        let mut out = YamlLoader::load_from_str(s).unwrap().into_iter();
        let mut doc = out.next().unwrap().into_iter();

        assert_eq!(doc.next().unwrap().into_string().unwrap(), "string");
        assert_eq!(doc.next().unwrap().into_string().unwrap(), "string");
        assert_eq!(doc.next().unwrap().into_string().unwrap(), "string");
        assert_eq!(doc.next().unwrap().into_i64().unwrap(), 123);
        assert_eq!(doc.next().unwrap().into_i64().unwrap(), -321);
        assert_eq!(doc.next().unwrap().into_f64().unwrap(), 1.23);
        assert_eq!(doc.next().unwrap().into_f64().unwrap(), -1e4);
        assert!(doc.next().unwrap().into_bool().unwrap());
        assert!(!doc.next().unwrap().into_bool().unwrap());
        assert_eq!(doc.next().unwrap().into_string().unwrap(), "0");
        assert_eq!(doc.next().unwrap().into_i64().unwrap(), 100);
        assert_eq!(doc.next().unwrap().into_f64().unwrap(), 2.0);
        assert!(doc.next().unwrap().into_bool().unwrap());
        assert!(!doc.next().unwrap().into_bool().unwrap());
        assert_eq!(doc.next().unwrap().into_i64().unwrap(), 255);
        assert_eq!(doc.next().unwrap().into_i64().unwrap(), 63);
        assert_eq!(doc.next().unwrap().into_i64().unwrap(), 12345);
        assert_eq!(doc.next().unwrap().into_f64().unwrap(), f64::NEG_INFINITY);
        assert!(doc.next().unwrap().into_f64().is_some());
        assert_eq!(doc.next().unwrap().into_f64().unwrap(), f64::INFINITY);
    }

    #[test]
    fn test_hash_order() {
        let s = "---
b: ~
a: ~
c: ~
";
        let out = YamlLoader::load_from_str(s).unwrap();
        let first = out.into_iter().next().unwrap();
        let mut iter = first.into_hash().unwrap().into_iter();
        assert_eq!(
            Some((Yaml::String("b".to_owned()), Yaml::Null)),
            iter.next()
        );
        assert_eq!(
            Some((Yaml::String("a".to_owned()), Yaml::Null)),
            iter.next()
        );
        assert_eq!(
            Some((Yaml::String("c".to_owned()), Yaml::Null)),
            iter.next()
        );
        assert_eq!(None, iter.next());
    }

    #[test]
    fn test_integer_key() {
        let s = "
0:
    important: true
1:
    important: false
";
        let out = YamlLoader::load_from_str(s).unwrap();
        let first = out.into_iter().next().unwrap();
        assert!(first[0]["important"].as_bool().unwrap());
    }

    #[test]
    fn test_indentation_equality() {
        let four_spaces = YamlLoader::load_from_str(
            r#"
hash:
    with:
        indentations
"#,
        )
        .unwrap()
        .into_iter()
        .next()
        .unwrap();

        let two_spaces = YamlLoader::load_from_str(
            r#"
hash:
  with:
    indentations
"#,
        )
        .unwrap()
        .into_iter()
        .next()
        .unwrap();

        let one_space = YamlLoader::load_from_str(
            r#"
hash:
 with:
  indentations
"#,
        )
        .unwrap()
        .into_iter()
        .next()
        .unwrap();

        let mixed_spaces = YamlLoader::load_from_str(
            r#"
hash:
     with:
               indentations
"#,
        )
        .unwrap()
        .into_iter()
        .next()
        .unwrap();

        assert_eq!(four_spaces, two_spaces);
        assert_eq!(two_spaces, one_space);
        assert_eq!(four_spaces, mixed_spaces);
    }

    #[test]
    fn test_two_space_indentations() {
        // https://github.com/kbknapp/clap-rs/issues/965

        let s = r#"
subcommands:
  - server:
    about: server related commands
subcommands2:
  - server:
      about: server related commands
subcommands3:
 - server:
    about: server related commands
            "#;

        let out = YamlLoader::load_from_str(s).unwrap();
        let doc = &out.into_iter().next().unwrap();

        println!("{:#?}", doc);
        assert_eq!(doc["subcommands"][0]["server"], Yaml::Null);
        assert!(doc["subcommands2"][0]["server"].as_hash().is_some());
        assert!(doc["subcommands3"][0]["server"].as_hash().is_some());
    }

    #[test]
    fn test_recursion_depth_check_objects() {
        let s = "{a:".repeat(10_000) + &"}".repeat(10_000);
        assert!(YamlLoader::load_from_str(&s).is_err());
    }

    #[test]
    fn test_recursion_depth_check_arrays() {
        let s = "[".repeat(10_000) + &"]".repeat(10_000);
        assert!(YamlLoader::load_from_str(&s).is_err());
    }

    #[test]
    fn test_or() {
        assert_eq!(Yaml::Null.or(Yaml::Integer(3)), Yaml::Integer(3));
        assert_eq!(Yaml::Integer(3).or(Yaml::Integer(7)), Yaml::Integer(3));
    }

    #[test]
    fn test_read_bom() {
        let s = b"\xef\xbb\xbf---
a: 1
b: 2.2
c: [1, 2]
";
        let out = YamlDecoder::read(s as &[u8]).decode().unwrap();
        let doc = &out[0];
        assert_eq!(doc["a"].as_i64().unwrap(), 1i64);
        assert_eq!(doc["b"].as_f64().unwrap(), 2.2f64);
        assert_eq!(doc["c"][1].as_i64().unwrap(), 2i64);
        assert!(doc["d"][0].is_badvalue());
    }

    #[test]
    fn test_read_utf16le() {
        let s = b"\xff\xfe-\x00-\x00-\x00
\x00a\x00:\x00 \x001\x00
\x00b\x00:\x00 \x002\x00.\x002\x00
\x00c\x00:\x00 \x00[\x001\x00,\x00 \x002\x00]\x00
\x00";
        let out = YamlDecoder::read(s as &[u8]).decode().unwrap();
        let doc = &out[0];
        println!("GOT: {:?}", doc);
        assert_eq!(doc["a"].as_i64().unwrap(), 1i64);
        assert_eq!(doc["b"].as_f64().unwrap(), 2.2f64);
        assert_eq!(doc["c"][1].as_i64().unwrap(), 2i64);
        assert!(doc["d"][0].is_badvalue());
    }

    #[test]
    fn test_read_utf16be() {
        let s = b"\xfe\xff\x00-\x00-\x00-\x00
\x00a\x00:\x00 \x001\x00
\x00b\x00:\x00 \x002\x00.\x002\x00
\x00c\x00:\x00 \x00[\x001\x00,\x00 \x002\x00]\x00
";
        let out = YamlDecoder::read(s as &[u8]).decode().unwrap();
        let doc = &out[0];
        println!("GOT: {:?}", doc);
        assert_eq!(doc["a"].as_i64().unwrap(), 1i64);
        assert_eq!(doc["b"].as_f64().unwrap(), 2.2f64);
        assert_eq!(doc["c"][1].as_i64().unwrap(), 2i64);
        assert!(doc["d"][0].is_badvalue());
    }

    #[test]
    fn test_read_utf16le_nobom() {
        let s = b"-\x00-\x00-\x00
\x00a\x00:\x00 \x001\x00
\x00b\x00:\x00 \x002\x00.\x002\x00
\x00c\x00:\x00 \x00[\x001\x00,\x00 \x002\x00]\x00
\x00";
        let out = YamlDecoder::read(s as &[u8]).decode().unwrap();
        let doc = &out[0];
        println!("GOT: {:?}", doc);
        assert_eq!(doc["a"].as_i64().unwrap(), 1i64);
        assert_eq!(doc["b"].as_f64().unwrap(), 2.2f64);
        assert_eq!(doc["c"][1].as_i64().unwrap(), 2i64);
        assert!(doc["d"][0].is_badvalue());
    }

    #[test]
    fn test_read_trap() {
        let s = b"---
a\xa9: 1
b: 2.2
c: [1, 2]
";
        let out = YamlDecoder::read(s as &[u8])
            .encoding_trap(encoding::DecoderTrap::Ignore)
            .decode()
            .unwrap();
        let doc = &out[0];
        println!("GOT: {:?}", doc);
        assert_eq!(doc["a"].as_i64().unwrap(), 1i64);
        assert_eq!(doc["b"].as_f64().unwrap(), 2.2f64);
        assert_eq!(doc["c"][1].as_i64().unwrap(), 2i64);
        assert!(doc["d"][0].is_badvalue());
    }

    struct HelloTagParser;

    impl YamlScalarParser for HelloTagParser {
        fn parse_scalar(&self, tag: &TokenType, value: &str) -> Option<Yaml> {
            if let TokenType::Tag(ref handle, ref suffix) = *tag {
                if (*handle == "!" || *handle == "yaml-rust.hello.prefix") && *suffix == "hello" {
                    return Some(Yaml::String("Hello ".to_string() + value));
                }
            }
            None
        }
    }

    #[test]
    fn test_scalar_parser() {
        let parser = HelloTagParser;
        let mut loader = YamlLoader::new();
        loader.register_scalar_parser(&parser);
        let out = loader.parse_from_str("!hello world").unwrap();
        let doc = &out[0];
        assert_eq!(doc.as_str().unwrap(), "Hello world")
    }

    #[test]
    fn test_tag_directive() {
        let parser = HelloTagParser;
        let mut loader = YamlLoader::new();
        loader.register_scalar_parser(&parser);
        let out = loader
            .parse_from_str(
                "%YAML 1.2
%TAG ! yaml-rust.hello.prefix:
---
!hello world",
            )
            .unwrap();
        let doc = &out[0];
        assert_eq!(doc.as_str().unwrap(), "Hello world")
    }
}
