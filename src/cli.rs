//! コマンドライン引数の解釈。
//!
//! 引数の意味づけ(何をすべきか)と、実際に `std::env::args()` を読む場所
//! (main.rs)を分けることで、引数解釈そのものを OS から独立してテストできる
//! ようにする。

use vmc_pet_body::{list_animals, MoodState};

/// 引数解釈の結果、main が実際に行うべきこと。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// ペットを起動する。`preview` があれば、その気分に固定したプレビューとして起動する。
    Run {
        animal_code: String,
        preview: Option<MoodState>,
    },
    /// 選べる生物の一覧を表示して終了する。
    ListAnimals,
    /// 【実験】多チャンネル Lenia の生物を、ペットの仕組みにつながずに表示する。
    PreviewMultichannel { id: String },
    /// 【実験】多チャンネル Lenia の生物を、元気・テンポ・クリックだけつないで動かす。
    /// `preview` があれば、その気分に固定する(テンポが変わる)。
    RunMultichannel {
        id: String,
        preview: Option<MoodState>,
    },
    /// 【実験】表示できる多チャンネルの生物の一覧を表示して終了する。
    ListMultichannel,
    /// 使い方を表示して終了する。
    Help,
}

/// 引数解釈に失敗した原因。
#[derive(Debug, PartialEq, Eq)]
pub enum ArgsError {
    UnknownFlag(String),
    MissingValue(&'static str),
    UnknownMood(String),
}

impl std::fmt::Display for ArgsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownFlag(flag) => write!(f, "unknown option: {flag}"),
            Self::MissingValue(flag) => write!(f, "{flag} requires a value"),
            Self::UnknownMood(value) => {
                let names: Vec<&str> = MoodState::ALL.iter().map(|state| state.name()).collect();
                write!(
                    f,
                    "unknown mood for --preview-mood: {value} (expected one of: {})",
                    names.join(", ")
                )
            }
        }
    }
}

/// `--preview-mood` の値を気分の状態として読む。
fn parse_mood(value: &str) -> Result<MoodState, ArgsError> {
    MoodState::from_name(value).ok_or_else(|| ArgsError::UnknownMood(value.to_string()))
}

impl std::error::Error for ArgsError {}

/// プログラム名を含まない引数列を解釈する。
pub fn parse(args: &[String], default_animal_code: &str) -> Result<Action, ArgsError> {
    let mut animal_code = default_animal_code.to_string();
    let mut preview = None;
    let mut multichannel: Option<String> = None;
    let mut iter = args.iter();

    while let Some(arg) = iter.next() {
        if arg == "-h" || arg == "--help" {
            return Ok(Action::Help);
        }
        if arg == "--list-animals" {
            return Ok(Action::ListAnimals);
        }
        if arg == "--list-multichannel" {
            return Ok(Action::ListMultichannel);
        }
        if arg == "--preview-multichannel" {
            let value = iter
                .next()
                .ok_or(ArgsError::MissingValue("--preview-multichannel"))?;
            return Ok(Action::PreviewMultichannel { id: value.clone() });
        }
        if let Some(value) = arg.strip_prefix("--preview-multichannel=") {
            return Ok(Action::PreviewMultichannel {
                id: value.to_string(),
            });
        }
        if arg == "--multichannel" {
            let value = iter
                .next()
                .ok_or(ArgsError::MissingValue("--multichannel"))?;
            multichannel = Some(value.clone());
            continue;
        }
        if let Some(value) = arg.strip_prefix("--multichannel=") {
            multichannel = Some(value.to_string());
            continue;
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
        if arg == "--preview-mood" {
            let value = iter
                .next()
                .ok_or(ArgsError::MissingValue("--preview-mood"))?;
            preview = Some(parse_mood(value)?);
            continue;
        }
        if let Some(value) = arg.strip_prefix("--preview-mood=") {
            preview = Some(parse_mood(value)?);
            continue;
        }
        return Err(ArgsError::UnknownFlag(arg.clone()));
    }

    if let Some(id) = multichannel {
        return Ok(Action::RunMultichannel { id, preview });
    }
    Ok(Action::Run {
        animal_code,
        preview,
    })
}

/// `--help` で表示する使い方。
pub const USAGE: &str = "\
vmc-pet [オプション]

オプション:
  --animal <code>    起動時に使う生物を指定する(デフォルト: O2u)
  --list-animals     選べる生物の一覧を表示して終了する
  --preview-mood <lively|waiting|disappointed>
                     気分を固定して、その見た目を確かめる(記憶は読みも書きもしない)
  --list-multichannel
                     【実験】表示できる多チャンネル Lenia の生物の一覧を表示して終了する
  --preview-multichannel <id>
                     【実験】多チャンネル Lenia の生物を 64x64 の場で表示する
                     (エネルギー・学習・記憶にはつながない。クリックで突ける)
  --multichannel <id>
                     【実験】多チャンネル Lenia の生物を、元気・テンポ・クリックだけつないで動かす
                     (--preview-mood と組み合わせると、その気分のテンポになる)
  -h, --help         このメッセージを表示して終了する
";

/// 表示できる多チャンネルの生物の一覧を人間向けに整形する。
pub fn format_multichannel_list() -> Result<String, serde_json::Error> {
    let animals = vmc_pet_body::multichannel::list_multichannel()?;
    let mut out = String::new();
    for animal in animals {
        let name = if animal.name.is_empty() {
            "(名前なし)"
        } else {
            &animal.name
        };
        out.push_str(&format!(
            "  {:7} {name} ({} チャンネル・カーネル {} 本)\n",
            animal.id,
            animal.cells.len(),
            animal.params.len()
        ));
    }
    Ok(out)
}

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
                animal_code: "O2u".to_string(),
                preview: None,
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
                animal_code: "OG2g".to_string(),
                preview: None,
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
                animal_code: "OG2g".to_string(),
                preview: None,
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
    fn preview_multichannel_takes_an_id_in_either_form() {
        // Arrange / Act / Assert
        let expected = Action::PreviewMultichannel {
            id: "221-09".to_string(),
        };
        assert_eq!(
            parse(&args(&["--preview-multichannel", "221-09"]), "O2u").unwrap(),
            expected
        );
        assert_eq!(
            parse(&args(&["--preview-multichannel=221-09"]), "O2u").unwrap(),
            expected
        );
    }

    #[test]
    fn a_dangling_preview_multichannel_flag_is_a_missing_value_error() {
        // Arrange / Act
        let result = parse(&args(&["--preview-multichannel"]), "O2u");

        // Assert
        assert_eq!(
            result,
            Err(ArgsError::MissingValue("--preview-multichannel"))
        );
    }

    #[test]
    fn multichannel_runs_the_body_and_can_be_combined_with_a_pinned_mood() {
        // Arrange / Act / Assert: 気分の指定はどちらの順でもよい
        assert_eq!(
            parse(&args(&["--multichannel", "231-04"]), "O2u").unwrap(),
            Action::RunMultichannel {
                id: "231-04".to_string(),
                preview: None,
            }
        );
        let expected = Action::RunMultichannel {
            id: "231-04".to_string(),
            preview: Some(MoodState::Disappointed),
        };
        assert_eq!(
            parse(
                &args(&["--preview-mood", "disappointed", "--multichannel=231-04"]),
                "O2u"
            )
            .unwrap(),
            expected
        );
        assert_eq!(
            parse(
                &args(&["--multichannel", "231-04", "--preview-mood=disappointed"]),
                "O2u"
            )
            .unwrap(),
            expected
        );
    }

    #[test]
    fn a_dangling_multichannel_flag_is_a_missing_value_error() {
        // Arrange / Act / Assert
        assert_eq!(
            parse(&args(&["--multichannel"]), "O2u"),
            Err(ArgsError::MissingValue("--multichannel"))
        );
    }

    #[test]
    fn list_multichannel_flag_is_recognized() {
        // Arrange / Act / Assert
        assert_eq!(
            parse(&args(&["--list-multichannel"]), "O2u").unwrap(),
            Action::ListMultichannel
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
    fn preview_mood_flag_selects_a_pinned_mood() {
        // Arrange / Act
        let separate = parse(&args(&["--preview-mood", "waiting"]), "O2u").unwrap();
        let joined = parse(&args(&["--preview-mood=disappointed"]), "O2u").unwrap();

        // Assert
        assert_eq!(
            separate,
            Action::Run {
                animal_code: "O2u".to_string(),
                preview: Some(MoodState::Waiting),
            }
        );
        assert_eq!(
            joined,
            Action::Run {
                animal_code: "O2u".to_string(),
                preview: Some(MoodState::Disappointed),
            }
        );
    }

    #[test]
    fn an_unknown_preview_mood_is_reported() {
        // Arrange / Act
        let result = parse(&args(&["--preview-mood", "sleepy"]), "O2u");

        // Assert
        assert_eq!(result, Err(ArgsError::UnknownMood("sleepy".to_string())));
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
