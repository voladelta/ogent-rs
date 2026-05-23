pub fn xml_escape(s: &str) -> String {
  let mut out = String::with_capacity(s.len());
  for c in s.chars() {
    match c {
      '&' => out.push_str("&amp;"),
      '"' => out.push_str("&quot;"),
      '<' => out.push_str("&lt;"),
      '>' => out.push_str("&gt;"),
      _ => out.push(c),
    }
  }
  out
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn test_xml_escape() {
    assert_eq!(xml_escape("hello"), "hello");
    assert_eq!(xml_escape("a & b"), "a &amp; b");
    assert_eq!(xml_escape("a < b"), "a &lt; b");
    assert_eq!(xml_escape("a > b"), "a &gt; b");
    assert_eq!(xml_escape("a \" b"), "a &quot; b");
    assert_eq!(
      xml_escape("<foo bar=\"baz\">&</foo>"),
      "&lt;foo bar=&quot;baz&quot;&gt;&amp;&lt;/foo&gt;"
    );
  }
}
