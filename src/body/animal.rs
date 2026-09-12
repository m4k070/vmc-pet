//! `assets/animals.json` の読み込みと、Lenia 独自 RLE 形式のデコード。
//!
//! データの出典とライセンスは `assets/NOTICE.md` を参照。

use serde::Deserialize;

use super::lenia::{GrowthMapping, KernelCore, LeniaParams};

/// 埋め込んだ生物データ。外部ファイルに依存させないため実行ファイルに含める。
const ANIMALS_JSON: &str = include_str!("../../assets/animals.json");

/// RLE の行区切り。2次元の Lenia では `$` が1行の終わりを表す。
const ROW_DELIMITER: char = '$';

/// 値の上位桁を表す接頭辞。これらの文字は次の1文字と組で1つの値になる。
const VALUE_PREFIXES: &str = "pqrstuvwxy@";

/// セル値の最大。RLE は 0..=255 の整数で値を持つ。
const MAX_ENCODED_VALUE: f32 = 255.0;

/// 生物の初期セル配置。行優先で並ぶ。
#[derive(Debug, Clone)]
pub struct Pattern {
    width: usize,
    height: usize,
    cells: Vec<f32>,
}

impl Pattern {
    pub fn width(&self) -> usize {
        self.width
    }

    pub fn height(&self) -> usize {
        self.height
    }

    /// 範囲外の座標は 0.0 を返す。
    pub fn get(&self, x: usize, y: usize) -> f32 {
        if x >= self.width || y >= self.height {
            return 0.0;
        }
        self.cells[y * self.width + x]
    }
}

/// animals.json 由来の生物1体。
#[derive(Debug, Clone)]
pub struct Animal {
    pub code: String,
    pub name: String,
    pub params: LeniaParams,
    pub pattern: Pattern,
}

/// 生物データを読み込めない原因。
#[derive(Debug)]
pub enum AnimalError {
    Parse(serde_json::Error),
    /// カーネル重み `b` が "1" や "1/2,1" の形式になっていない。
    KernelPeaks(String),
    /// `kn` / `gn` が既知の関数を指していない。
    UnknownFunction(&'static str, u32),
    /// RLE に解釈できない文字が含まれている。
    Rle(char),
    /// 要求した生物が見つからない。
    NotFound(String),
}

impl std::fmt::Display for AnimalError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Parse(e) => write!(f, "failed to parse animals.json: {e}"),
            Self::KernelPeaks(raw) => write!(f, "invalid kernel peaks {raw:?} (expected e.g. \"1\" or \"1/2,1\")"),
            Self::UnknownFunction(name, index) => write!(f, "unknown {name} index: {index}"),
            Self::Rle(ch) => write!(f, "unexpected character in the rle payload: {ch:?}"),
            Self::NotFound(code) => write!(f, "no animal with code {code:?}"),
        }
    }
}

impl std::error::Error for AnimalError {}

/// 埋め込まれた生物データから、コードが一致する1体を読み込む。
pub fn load_animal(code: &str) -> Result<Animal, AnimalError> {
    let document: AnimalsDocument =
        serde_json::from_str(ANIMALS_JSON).map_err(AnimalError::Parse)?;
    let entry = document
        .animals
        .into_iter()
        .find(|entry| entry.code == code)
        .ok_or_else(|| AnimalError::NotFound(code.to_string()))?;

    let params = to_lenia_params(&entry.params)?;
    let pattern = decode_rle(&entry.cells)?;
    Ok(Animal {
        code: entry.code,
        name: entry.name,
        params,
        pattern,
    })
}

/// 埋め込まれた生物の一覧を `(code, name)` で返す。
/// CLI の `--list-animals` など、選べる生物を人間に提示する場面で使う。
pub fn list_animals() -> Result<Vec<(String, String)>, AnimalError> {
    let document: AnimalsDocument =
        serde_json::from_str(ANIMALS_JSON).map_err(AnimalError::Parse)?;
    Ok(document
        .animals
        .into_iter()
        .map(|entry| (entry.code, entry.name))
        .collect())
}

#[derive(Deserialize)]
struct AnimalsDocument {
    animals: Vec<AnimalEntry>,
}

#[derive(Deserialize)]
struct AnimalEntry {
    code: String,
    name: String,
    params: RawParams,
    cells: String,
}

#[derive(Deserialize)]
struct RawParams {
    #[serde(rename = "R")]
    radius: usize,
    #[serde(rename = "T")]
    time_divisor: f32,
    b: String,
    m: f32,
    s: f32,
    kn: u32,
    gn: u32,
}

fn to_lenia_params(raw: &RawParams) -> Result<LeniaParams, AnimalError> {
    Ok(LeniaParams {
        radius: raw.radius,
        time_divisor: raw.time_divisor,
        kernel_peaks: parse_kernel_peaks(&raw.b)?,
        growth_center: raw.m,
        growth_width: raw.s,
        kernel_core: KernelCore::from_index(raw.kn)
            .ok_or(AnimalError::UnknownFunction("kernel core", raw.kn))?,
        growth_mapping: GrowthMapping::from_index(raw.gn)
            .ok_or(AnimalError::UnknownFunction("growth mapping", raw.gn))?,
    })
}

/// `b` は "1" や "1/2,1" のように、カンマ区切りの分数で書かれる。
fn parse_kernel_peaks(raw: &str) -> Result<Vec<f32>, AnimalError> {
    let mut peaks = Vec::new();
    for term in raw.split(',') {
        let value = match term.split_once('/') {
            None => term.trim().parse::<f32>().ok(),
            Some((numerator, denominator)) => {
                match (
                    numerator.trim().parse::<f32>(),
                    denominator.trim().parse::<f32>(),
                ) {
                    (Ok(n), Ok(d)) if d != 0.0 => Some(n / d),
                    _ => None,
                }
            }
        };
        peaks.push(value.ok_or_else(|| AnimalError::KernelPeaks(raw.to_string()))?);
    }
    if peaks.is_empty() {
        return Err(AnimalError::KernelPeaks(raw.to_string()));
    }
    Ok(peaks)
}

/// Lenia 独自の RLE をデコードする。
///
/// 数字は直後のトークンの繰り返し回数、`pqrstuvwxy@` は次の1文字と組で1つの値、
/// `$` は行の終わりを表す。行の長さは最大行に合わせて 0 で埋める。
fn decode_rle(encoded: &str) -> Result<Pattern, AnimalError> {
    let mut rows: Vec<Vec<f32>> = Vec::new();
    let mut row: Vec<f32> = Vec::new();
    let mut prefix: Option<char> = None;
    let mut count = String::new();

    // 末尾の `!` を落とし、最終行を閉じるための区切りを足す
    let payload = format!("{}{}", encoded.trim_end_matches('!'), ROW_DELIMITER);

    for ch in payload.chars() {
        if ch.is_ascii_digit() {
            count.push(ch);
            continue;
        }
        if prefix.is_none() && VALUE_PREFIXES.contains(ch) {
            prefix = Some(ch);
            continue;
        }

        let repeat = if count.is_empty() {
            1
        } else {
            count.parse::<usize>().unwrap_or(1)
        };

        if ch == ROW_DELIMITER && prefix.is_none() {
            rows.push(std::mem::take(&mut row));
            // 行区切りに付いた回数は、空行がその数だけ続くことを意味する
            for _ in 1..repeat {
                rows.push(Vec::new());
            }
        } else {
            let value = decode_value(prefix, ch)? / MAX_ENCODED_VALUE;
            row.extend(std::iter::repeat_n(value, repeat));
        }

        prefix = None;
        count.clear();
    }

    let width = rows.iter().map(Vec::len).max().unwrap_or(0);
    let height = rows.len();
    let mut cells = Vec::with_capacity(width * height);
    for row in &rows {
        cells.extend_from_slice(row);
        cells.extend(std::iter::repeat_n(0.0, width - row.len()));
    }

    Ok(Pattern {
        width,
        height,
        cells,
    })
}

/// 1トークンを 0..=255 の値に変換する。
fn decode_value(prefix: Option<char>, ch: char) -> Result<f32, AnimalError> {
    const PREFIX_SPAN: u32 = 24;
    const PREFIX_BASE: u32 = 25;

    match prefix {
        None => match ch {
            '.' | 'b' => Ok(0.0),
            'o' => Ok(MAX_ENCODED_VALUE),
            'A'..='Z' => Ok((ch as u32 - 'A' as u32 + 1) as f32),
            _ => Err(AnimalError::Rle(ch)),
        },
        Some(prefix) => {
            if !ch.is_ascii_uppercase() {
                return Err(AnimalError::Rle(ch));
            }
            let high = (prefix as u32).saturating_sub('p' as u32) * PREFIX_SPAN;
            let low = ch as u32 - 'A' as u32 + PREFIX_BASE;
            Ok((high + low) as f32)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_value_matches_the_reference_encoding() {
        // Arrange / Act / Assert: 空セル・単独大文字・接頭辞つきの3系統
        assert_eq!(decode_value(None, '.').unwrap(), 0.0);
        assert_eq!(decode_value(None, 'M').unwrap(), 13.0);
        assert_eq!(decode_value(Some('q'), 'L').unwrap(), 60.0);
    }

    #[test]
    fn decode_rle_expands_counts_and_rows() {
        // Arrange: 1行目は空セル3つと値、2行目は値の繰り返し
        let encoded = "3.M$2A!";

        // Act
        let pattern = decode_rle(encoded).unwrap();

        // Assert
        assert_eq!(pattern.width(), 4);
        assert_eq!(pattern.height(), 2);
        assert_eq!(pattern.get(0, 0), 0.0);
        assert!((pattern.get(3, 0) - 13.0 / 255.0).abs() < f32::EPSILON);
        assert!((pattern.get(1, 1) - 1.0 / 255.0).abs() < f32::EPSILON);
        // 短い行は 0 で埋められる
        assert_eq!(pattern.get(3, 1), 0.0);
    }

    #[test]
    fn parse_kernel_peaks_accepts_fractions() {
        // Arrange / Act
        let single = parse_kernel_peaks("1").unwrap();
        let multi = parse_kernel_peaks("1/2,1").unwrap();

        // Assert
        assert_eq!(single, vec![1.0]);
        assert_eq!(multi, vec![0.5, 1.0]);
    }

    #[test]
    fn parse_kernel_peaks_rejects_garbage() {
        // Arrange / Act
        let result = parse_kernel_peaks("1,oops");

        // Assert
        assert!(result.is_err());
    }

    #[test]
    fn orbium_loads_with_the_documented_shape() {
        // Arrange / Act
        let animal = load_animal("O2u").unwrap();

        // Assert
        assert_eq!(animal.name, "Orbium unicaudatus");
        assert_eq!(animal.params.radius, 13);
        assert_eq!(animal.params.kernel_peaks, vec![1.0]);
        assert_eq!(animal.pattern.width(), 20);
        assert_eq!(animal.pattern.height(), 20);
    }

    #[test]
    fn list_animals_includes_every_embedded_code() {
        // Arrange / Act
        let animals = list_animals().unwrap();

        // Assert
        assert_eq!(animals.len(), 4, "expected all 4 vendored animals to be listed");
        assert!(animals.iter().any(|(code, _)| code == "O2u"));
        for code in ["O2u", "OG2g", "S1s", "2S1v"] {
            assert!(
                animals.iter().any(|(c, _)| c == code),
                "list_animals must include {code}"
            );
        }
    }

    #[test]
    fn every_listed_code_actually_loads() {
        // Arrange
        let animals = list_animals().unwrap();

        // Act / Assert: 一覧に出す以上、実際に読み込めなければならない
        for (code, _name) in animals {
            assert!(load_animal(&code).is_ok(), "listed code {code} must load");
        }
    }

    #[test]
    fn load_animal_reports_an_unknown_code() {
        // Arrange / Act
        let result = load_animal("NOPE");

        // Assert
        assert!(matches!(result, Err(AnimalError::NotFound(_))));
    }
}
