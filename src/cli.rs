//! コマンドライン引数の解釈。
//!
//! 引数の意味づけ(何をすべきか)と、実際に `std::env::args()` を読む場所
//! (main.rs)を分けることで、引数解釈そのものを OS から独立してテストできる
//! ようにする。

use vmc_pet_body::list_animals;

/// 引数解釈の結果、main が実際に行うべきこと。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// ペットを起動する。
    Run { animal_code: String },
    /// 選べる生物の一覧を表示して終了する。
    ListAnimals,
    /// 使い方を表示して終了する。
    Help,
}

/// 引数解釈に失敗した原因。
#[derive(Debug, PartialEq, Eq)]
pub enum ArgsError {
    UnknownFlag(String),
    MissingValue(&'static str),
}

impl std::fmt::Display for ArgsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownFlag(flag) => write!(f, "unknown option: {flag}"),
            Self::MissingValue(flag) => write!(f, "{flag} requires a value"),
        }
    }
}

impl std::error::Error for ArgsError {}

/// プログラム名を含まない引数列を解釈する。
pub fn parse(args: &[String], default_animal_code: &str) -> Result<Action, ArgsError> {
    let mut animal_code = default_animal_code.to_string();
    let mut iter = args.iter();

    while let Some(arg) = iter.next() {
        if arg == "-h" || arg == "--help" {
            return Ok(Action::Help);
        }
        if arg == "--list-animals" {
            return Ok(Action::ListAnimals);
        }
        if arg == "--animal" {
            let value = iter.next().ok_or(ArgsError::MissingValue("--animal"))?;
            animal_code = value.clone();
            continue;
        }
        if let Some(value) = arg.strip_prefix("--animal=") {
            animal_code = value.to_string();
            continue;
        }
        return Err(ArgsError::UnknownFlag(arg.clone()));
    }

    Ok(Action::Run { animal_code })
}

/// `--help` で表示する使い方。
pub const USAGE: &str = "\
vmc-pet [オプション]

オプション:
  --animal <code>    起動時に使う生物を指定する(デフォルト: O2u)
  --list-animals     選べる生物の一覧を表示して終了する
  -h, --help         このメッセージを表示して終了する
";

/// 選べる生物の一覧を人間向けに整形する。
pub fn format_animal_list() -> Result<String, vmc_pet_body::animal::AnimalError> {
    let animals = list_animals()?;
    let mut out = String::new();
    for (code, name) in animals {
        out.push_str(&format!("  {code:6}  {name}\n"));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn no_arguments_runs_with_the_default_animal() {
        // Arrange / Act
        let action = parse(&args(&[]), "O2u").unwrap();

        // Assert
        assert_eq!(
            action,
            Action::Run {
                animal_code: "O2u".to_string()
            }
        );
    }

    #[test]
    fn animal_flag_with_a_separate_value_overrides_the_default() {
        // Arrange / Act
        let action = parse(&args(&["--animal", "OG2g"]), "O2u").unwrap();

        // Assert
        assert_eq!(
            action,
            Action::Run {
                animal_code: "OG2g".to_string()
            }
        );
    }

    #[test]
    fn animal_flag_with_an_equals_sign_also_works() {
        // Arrange / Act
        let action = parse(&args(&["--animal=OG2g"]), "O2u").unwrap();

        // Assert
        assert_eq!(
            action,
            Action::Run {
                animal_code: "OG2g".to_string()
            }
        );
    }

    #[test]
    fn help_flags_take_priority_over_everything_else() {
        // Arrange / Act / Assert
        assert_eq!(parse(&args(&["-h"]), "O2u").unwrap(), Action::Help);
        assert_eq!(parse(&args(&["--help"]), "O2u").unwrap(), Action::Help);
        assert_eq!(
            parse(&args(&["--animal", "OG2g", "--help"]), "O2u").unwrap(),
            Action::Help
        );
    }

    #[test]
    fn list_animals_flag_is_recognized() {
        // Arrange / Act
        let action = parse(&args(&["--list-animals"]), "O2u").unwrap();

        // Assert
        assert_eq!(action, Action::ListAnimals);
    }

    #[test]
    fn a_dangling_animal_flag_is_a_missing_value_error() {
        // Arrange / Act
        let result = parse(&args(&["--animal"]), "O2u");

        // Assert
        assert_eq!(result, Err(ArgsError::MissingValue("--animal")));
    }

    #[test]
    fn an_unrecognized_flag_is_reported() {
        // Arrange / Act
        let result = parse(&args(&["--nope"]), "O2u");

        // Assert
        assert_eq!(result, Err(ArgsError::UnknownFlag("--nope".to_string())));
    }

    #[test]
    fn format_animal_list_includes_every_code() {
        // Arrange / Act
        let list = format_animal_list().unwrap();

        // Assert
        for code in ["O2u", "OG2g", "S1s", "2S1v"] {
            assert!(list.contains(code), "expected {code} in:\n{list}");
        }
    }
}
