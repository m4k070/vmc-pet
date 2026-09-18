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
    /// `particles` があれば、Lenia の代わりに粒子の体(seed)で起動する。
    Run {
        animal_code: String,
        preview: Option<MoodState>,
        particle_seed: Option<u64>,
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
    /// 【実験】粒子の体(探索の候補番号 `seed`)を、`zoom` 倍に拡大して表示する。
    /// `log` があれば、1 秒ごとの体の状態とユーザーの操作をそのファイルへ書き出す。
    PreviewParticles {
        seed: u64,
        zoom: u32,
        log: Option<String>,
        /// コントローラ(まとまりが崩れたら誘う)を働かせるか。
        controller: bool,
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
    /// 数を取るオプションに、受け付けない値が渡された(候補番号は 0 以上、倍率は 1 以上の整数)。
    InvalidNumber(&'static str, String),
    /// 決まった語を取るオプションに、知らない値が渡された。
    UnknownValue(&'static str, String),
}

impl std::fmt::Display for ArgsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownFlag(flag) => write!(f, "unknown option: {flag}"),
            Self::MissingValue(flag) => write!(f, "{flag} requires a value"),
            Self::InvalidNumber(flag, value) => {
                write!(
                    f,
                    "{flag} expects a non-negative integer (zoom: 1 or more), got: {value}"
                )
            }
            Self::UnknownValue(flag, value) => {
                write!(f, "{flag} expects on or off, got: {value}")
            }
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

/// `minimum` 以上の整数を読む。
fn parse_at_least(flag: &'static str, value: &str, minimum: u64) -> Result<u64, ArgsError> {
    match value.parse::<u64>() {
        Ok(number) if number >= minimum => Ok(number),
        _ => Err(ArgsError::InvalidNumber(flag, value.to_string())),
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
    let mut particles: Option<u64> = None;
    let mut particle_zoom: u32 = 1;
    let mut particle_log: Option<String> = None;
    let mut particle_controller = true;
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
        if let Some((flag, value)) = split_flag(arg, &mut iter, "--preview-particles")? {
            particles = Some(parse_at_least(flag, &value, 0)?);
            continue;
        }
        if let Some((flag, value)) = split_flag(arg, &mut iter, "--particle-zoom")? {
            particle_zoom = parse_at_least(flag, &value, 1)?.min(8) as u32;
            continue;
        }
        if let Some((_, value)) = split_flag(arg, &mut iter, "--particle-log")? {
            particle_log = Some(value);
            continue;
        }
        if let Some((flag, value)) = split_flag(arg, &mut iter, "--particle-controller")? {
            particle_controller = match value.as_str() {
                "on" => true,
                "off" => false,
                _ => return Err(ArgsError::UnknownValue(flag, value)),
            };
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

    if let Some(seed) = particles {
        return Ok(Action::PreviewParticles {
            seed,
            zoom: particle_zoom,
            log: particle_log,
            controller: particle_controller,
        });
    }
    if let Some(id) = multichannel {
        return Ok(Action::RunMultichannel { id, preview });
    }
    // `particles:<番号>`(<code> の値)は、粒子の体(探索の候補番号)として読む。
    // 記憶・気分・echo・色素など、Lenia のペットと同じ仕組みに繋がる
    if let Some(seed_text) = animal_code.strip_prefix("particles:") {
        let seed = seed_text
            .parse::<u64>()
            .map_err(|_| ArgsError::InvalidNumber("--animal", seed_text.to_string()))?;
        return Ok(Action::Run {
            animal_code,
            preview,
            particle_seed: Some(seed),
        });
    }
    Ok(Action::Run {
        animal_code,
        preview,
        particle_seed: None,
    })
}

/// `flag <値>` と `flag=<値>` のどちらでも値を取り出す。`arg` が `flag` でなければ `None`。
fn split_flag<'a>(
    arg: &str,
    iter: &mut impl Iterator<Item = &'a String>,
    flag: &'static str,
) -> Result<Option<(&'static str, String)>, ArgsError> {
    if arg == flag {
        let value = iter.next().ok_or(ArgsError::MissingValue(flag))?;
        return Ok(Some((flag, value.clone())));
    }
    Ok(arg
        .strip_prefix(flag)
        .and_then(|rest| rest.strip_prefix('='))
        .map(|value| (flag, value.to_string())))
}

/// `--help` で表示する使い方。
pub const USAGE: &str = "\
vmc-pet [オプション]

オプション:
  --animal <code>    起動時に使う生物を指定する(デフォルト: O2u)。
                     particles:<番号>(例 particles:1091)を渡すと、粒子の体で起動する
                     (エネルギー・気分・echo・色素・慣れ・記憶は Lenia と同じ仕組み)
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
  --preview-particles <番号> [--particle-zoom <倍率>] [--particle-log <ファイル>]
                     [--particle-controller <on|off>]
                     【実験】粒子の体(探索の候補番号、例 1091)を表示する。倍率を上げると体は大きく
                     見えるが、動き回れる箱は狭くなる(ポインタは撫でるだけ、クリックで弾く)。
                     誘いはコントローラが行い、--particle-controller off で止められる。
                     --particle-log を渡すと、1 秒ごとの体の状態と操作を CSV で書き出す
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
                particle_seed: None,
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
                particle_seed: None,
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
                particle_seed: None,
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
    fn preview_particles_takes_a_seed_and_an_optional_zoom() {
        // Arrange / Act / Assert
        assert_eq!(
            parse(&args(&["--preview-particles", "1091"]), "O2u").unwrap(),
            Action::PreviewParticles {
                seed: 1091,
                zoom: 1,
                log: None,
                controller: true
            }
        );
        assert_eq!(
            parse(
                &args(&["--particle-zoom=2", "--preview-particles=1937"]),
                "O2u"
            )
            .unwrap(),
            Action::PreviewParticles {
                seed: 1937,
                zoom: 2,
                log: None,
                controller: true
            }
        );
    }

    #[test]
    fn particle_log_takes_a_path() {
        // Arrange / Act / Assert
        assert_eq!(
            parse(
                &args(&[
                    "--preview-particles",
                    "1091",
                    "--particle-log",
                    "/tmp/a.csv"
                ]),
                "O2u"
            )
            .unwrap(),
            Action::PreviewParticles {
                seed: 1091,
                zoom: 1,
                log: Some("/tmp/a.csv".to_string()),
                controller: true,
            }
        );
    }

    #[test]
    fn the_particle_controller_can_be_turned_off() {
        // Arrange / Act / Assert
        assert_eq!(
            parse(
                &args(&["--preview-particles=1091", "--particle-controller=off"]),
                "O2u"
            )
            .unwrap(),
            Action::PreviewParticles {
                seed: 1091,
                zoom: 1,
                log: None,
                controller: false,
            }
        );
        assert_eq!(
            parse(
                &args(&["--preview-particles=1091", "--particle-controller", "yes"]),
                "O2u"
            ),
            Err(ArgsError::UnknownValue(
                "--particle-controller",
                "yes".to_string()
            ))
        );
    }

    #[test]
    fn a_non_numeric_particle_option_is_reported() {
        // Arrange / Act / Assert
        assert_eq!(
            parse(&args(&["--preview-particles", "abc"]), "O2u"),
            Err(ArgsError::InvalidNumber(
                "--preview-particles",
                "abc".to_string()
            ))
        );
        assert_eq!(
            parse(
                &args(&["--preview-particles", "1091", "--particle-zoom", "0"]),
                "O2u"
            ),
            Err(ArgsError::InvalidNumber("--particle-zoom", "0".to_string()))
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
                particle_seed: None,
            }
        );
        assert_eq!(
            joined,
            Action::Run {
                animal_code: "O2u".to_string(),
                preview: Some(MoodState::Disappointed),
                particle_seed: None,
            }
        );
    }

    #[test]
    fn an_animal_code_with_the_particles_prefix_selects_a_particle_seed() {
        // Arrange / Act
        let action = parse(&args(&["--animal=particles:1091"]), "O2u").unwrap();

        // Assert
        assert_eq!(
            action,
            Action::Run {
                animal_code: "particles:1091".to_string(),
                preview: None,
                particle_seed: Some(1091),
            }
        );
    }

    #[test]
    fn a_malformed_particles_seed_is_reported() {
        // Arrange / Act
        let result = parse(&args(&["--animal=particles:abc"]), "O2u");

        // Assert
        assert_eq!(
            result,
            Err(ArgsError::InvalidNumber("--animal", "abc".to_string()))
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
