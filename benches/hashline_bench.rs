use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};

const HEX: &[u8; 16] = b"0123456789abcdef";

mod original {
  pub fn source_lines(source: &str) -> Vec<String> {
    let has_trailing = source.ends_with('\n');
    let s = if has_trailing {
      &source[..source.len() - 1]
    } else {
      source
    };
    if s.is_empty() {
      return Vec::new();
    }
    s.split('\n').map(String::from).collect()
  }

  pub fn render_hashlines(source: &str, start: Option<usize>, end: Option<usize>) -> String {
    let lines = source_lines(source);
    let slice_start = start
      .map(|s| if s > 0 { s - 1 } else { 0 })
      .unwrap_or(0)
      .min(lines.len());
    let slice_end = end.unwrap_or(lines.len()).min(lines.len());
    lines[slice_start..slice_end]
      .iter()
      .enumerate()
      .map(|(i, line)| {
        let line_no = slice_start + i + 1;
        let hash = line_hash(line);
        format!("{line_no}:{hash}|{line}\n")
      })
      .collect()
  }

  fn line_hash(line: &str) -> String {
    format!("{:04x}", fnv1a64(line.as_bytes()) >> 48)
  }

  fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for b in bytes {
      hash ^= u64::from(*b);
      hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    hash
  }
}

mod optimized {
  use super::HEX;

  pub fn render_hashlines(source: &str, start: Option<usize>, end: Option<usize>) -> String {
    let lines = source_lines_ref(source);
    let slice_start = start
      .map(|s| if s > 0 { s - 1 } else { 0 })
      .unwrap_or(0)
      .min(lines.len());
    let slice_end = end.unwrap_or(lines.len()).min(lines.len());
    let slice = &lines[slice_start..slice_end];
    let estimated: usize = slice.iter().map(|l| l.len() + 12).sum();
    let mut out = String::with_capacity(estimated);
    let mut hbuf = [0u8; 4];
    for (i, line) in slice.iter().enumerate() {
      let line_no = slice_start + i + 1;
      line_hash_into(line, &mut hbuf);
      push_usize(&mut out, line_no);
      out.push(':');
      out.push_str(std::str::from_utf8(&hbuf).unwrap());
      out.push('|');
      out.push_str(line);
      out.push('\n');
    }
    out
  }

  fn push_usize(out: &mut String, mut n: usize) {
    if n == 0 {
      out.push('0');
      return;
    }
    let mut buf = [0u8; 20];
    let mut pos = 20;
    while n > 0 {
      pos -= 1;
      buf[pos] = b'0' + (n % 10) as u8;
      n /= 10;
    }
    out.push_str(std::str::from_utf8(&buf[pos..]).unwrap());
  }

  fn source_lines_ref(source: &str) -> Vec<&str> {
    let s = source.strip_suffix('\n').unwrap_or(source);
    if s.is_empty() {
      return Vec::new();
    }
    s.split('\n').collect()
  }

  #[inline]
  fn line_hash_into(line: &str, buf: &mut [u8; 4]) {
    let h = (fnv1a64(line.as_bytes()) >> 48) as u16;
    buf[0] = HEX[((h >> 12) & 0xf) as usize];
    buf[1] = HEX[((h >> 8) & 0xf) as usize];
    buf[2] = HEX[((h >> 4) & 0xf) as usize];
    buf[3] = HEX[(h & 0xf) as usize];
  }

  fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for b in bytes {
      hash ^= u64::from(*b);
      hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    hash
  }
}

fn generate_source(lines: usize) -> String {
  let mut out = String::with_capacity(lines * 60);
  for i in 0..lines {
    match i % 5 {
      0 => out.push_str(
        "fn process_data(input: &str, config: &Config) -> Result<Vec<Item>> {\n",
      ),
      1 => out.push_str("    let parsed = parse(input)?;\n"),
      2 => out.push('\n'),
      3 => out.push_str("    Ok(items)\n"),
      _ => out.push_str(
        "    let items: Vec<Item> = parsed.iter().filter_map(|p| transform(p, config)).collect();\n",
      ),
    }
  }
  out
}

fn bench_render_hashlines(c: &mut Criterion) {
  let source = generate_source(500);

  let mut group = c.benchmark_group("render_hashlines_500");
  group.bench_function("original", |b| {
    b.iter(|| original::render_hashlines(black_box(&source), None, None))
  });
  group.bench_function("optimized", |b| {
    b.iter(|| optimized::render_hashlines(black_box(&source), None, None))
  });
  group.finish();
}

fn bench_render_hashlines_by_size(c: &mut Criterion) {
  let mut group = c.benchmark_group("render_hashlines_scaling");
  for size in [100, 500, 2000] {
    let source = generate_source(size);
    group.bench_with_input(BenchmarkId::new("original", size), &source, |b, src| {
      b.iter(|| original::render_hashlines(black_box(src), None, None))
    });
    group.bench_with_input(BenchmarkId::new("optimized", size), &source, |b, src| {
      b.iter(|| optimized::render_hashlines(black_box(src), None, None))
    });
  }
  group.finish();
}

fn bench_render_with_range(c: &mut Criterion) {
  let source = generate_source(2000);
  let mut group = c.benchmark_group("render_hashlines_range");
  group.bench_function("original_100_200", |b| {
    b.iter(|| original::render_hashlines(black_box(&source), Some(100), Some(200)))
  });
  group.bench_function("optimized_100_200", |b| {
    b.iter(|| optimized::render_hashlines(black_box(&source), Some(100), Some(200)))
  });
  group.finish();
}

criterion_group!(
  benches,
  bench_render_hashlines,
  bench_render_hashlines_by_size,
  bench_render_with_range,
);
criterion_main!(benches);
