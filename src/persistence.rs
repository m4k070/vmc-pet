//! 記憶(`vmc_pet_body::memory`)をファイルシステムと時計に繋ぐ、PC版固有の層。
//!
//! 「何を持ち越すか」は体の側(共有クレート)が決める。ここが持つのは
//! 「どこへ、どの形式で置くか」と「いまが何時か」だけで、これはどちらも
//! OS 依存の話であり、M5Stack 版では別の実装(RTC とフラッシュ)になる。
//!
//! 記憶が読めない・書けないことは、ペットが生きられない理由にはしない。
//! 初回起動にはそもそもファイルが無いし、壊れたファイルのせいでペットが
//! 起動しなくなるのは本末転倒なので、失敗しても記録して先へ進む。

use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use vmc_pet_body::{PetMemory, SavedMemory};

/// 保存ファイルの置き場所を XDG Base Directory 仕様に従って決める。
/// `XDG_STATE_HOME` が無ければ `~/.local/state` を使う。
fn state_path() -> Option<PathBuf> {
    let base = match std::env::var_os("XDG_STATE_HOME") {
        Some(dir) if !dir.is_empty() => PathBuf::from(dir),
        _ => PathBuf::from(std::env::var_os("HOME")?).join(".local/state"),
    };
    Some(base.join("vmc-pet").join("state.json"))
}

/// いまの Unix 時刻(秒)。時計が UNIX_EPOCH より前を指している異常時は 0 を返す。
pub fn now_unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0)
}

/// 記憶の読み書き。
pub struct MemoryStore {
    path: Option<PathBuf>,
    /// 書き込み失敗のログを一度だけ出すためのフラグ
    /// (`interface/machine_load.rs` と同じ扱い方)。
    warned: bool,
}

impl MemoryStore {
    pub fn new() -> Self {
        Self {
            path: state_path(),
            warned: false,
        }
    }

    /// 保存先を明示して作る。テスト(実際のユーザーの状態ディレクトリを
    /// 汚さずに往復を確かめる)専用。
    #[cfg(test)]
    pub fn at(path: PathBuf) -> Self {
        Self {
            path: Some(path),
            warned: false,
        }
    }

    /// 何も読まず、何も書かない store。テスト専用。
    ///
    /// 記憶を読むようになった結果、テストが「実行環境に既にある保存ファイル」に
    /// 左右される(実際にペットを動かした後だけ失敗する)ようになってはいけない
    /// ため、テストからは必ずこれを渡す。
    #[cfg(test)]
    pub fn disabled() -> Self {
        Self {
            path: None,
            warned: false,
        }
    }

    /// 保存されている記憶を読む。初回起動(ファイルが無い)や、読めなかった
    /// 場合は `None` を返す。
    pub fn load(&self) -> Option<SavedMemory> {
        let path = self.path.as_ref()?;
        let raw = match fs::read_to_string(path) {
            Ok(raw) => raw,
            // 初回起動ではファイルが無いのが正常なので、これは警告しない
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return None,
            Err(error) => {
                eprintln!("vmc-pet: could not read {}: {error}", path.display());
                return None;
            }
        };
        match serde_json::from_str(&raw) {
            Ok(saved) => Some(saved),
            Err(error) => {
                eprintln!("vmc-pet: ignoring a broken memory file {}: {error}", path.display());
                None
            }
        }
    }

    /// 記憶を書き出す。失敗しても動作は続け、警告は一度だけ出す。
    pub fn save(&mut self, memory: PetMemory) {
        let Some(path) = self.path.clone() else {
            return;
        };
        if let Err(error) = self.write(&path, memory) {
            if !self.warned {
                eprintln!("vmc-pet: could not save memory to {}: {error}", path.display());
                self.warned = true;
            }
        }
    }

    fn write(&self, path: &PathBuf, memory: PetMemory) -> Result<(), String> {
        let saved = SavedMemory::new(memory, now_unix_seconds());
        let serialized =
            serde_json::to_string_pretty(&saved).map_err(|error| format!("serialize: {error}"))?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .map_err(|error| format!("create_dir_all({}): {error}", parent.display()))?;
        }
        fs::write(path, serialized).map_err(|error| format!("write: {error}"))
    }
}

impl Default for MemoryStore {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_path_follows_xdg_state_home_when_set() {
        // Arrange / Act / Assert: 環境変数を触るテストは他のテストと干渉しうるため、
        // ここでは経路の形だけを確かめる(実際の値は実行環境に依存する)
        let path = state_path().expect("HOME or XDG_STATE_HOME must exist in the test environment");
        assert!(path.ends_with("vmc-pet/state.json"), "got {}", path.display());
    }

    #[test]
    fn now_unix_seconds_is_a_plausible_timestamp() {
        // Arrange / Act
        let now = now_unix_seconds();

        // Assert: 2020-01-01 より後であること(時計が壊れていない限り成り立つ)
        assert!(now > 1_577_836_800, "got {now}");
    }

    /// テスト用の一時パス。実際のユーザーの状態ディレクトリは触らない。
    fn temp_path(name: &str) -> PathBuf {
        std::env::temp_dir()
            .join(format!("vmc-pet-test-{}-{}", std::process::id(), name))
            .join("state.json")
    }

    #[test]
    fn saving_then_loading_returns_the_same_memory() {
        // Arrange
        let path = temp_path("round-trip");
        let mut store = MemoryStore::at(path.clone());
        let memory = PetMemory { energy: 0.42 };

        // Act
        store.save(memory);
        let loaded = store.load().expect("just-saved memory must load");

        // Assert
        assert_eq!(loaded.memory, memory);
        assert!(loaded.seconds_away(now_unix_seconds()) < 5.0, "saved just now");

        let _ = fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn a_missing_file_is_not_an_error() {
        // Arrange: まだ何も保存していない場所(初回起動に相当)
        let store = MemoryStore::at(temp_path("missing"));

        // Act / Assert
        assert!(store.load().is_none());
    }

    #[test]
    fn a_broken_file_is_ignored_rather_than_fatal() {
        // Arrange: 壊れた内容を書いておく
        let path = temp_path("broken");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "this is not json").unwrap();
        let store = MemoryStore::at(path.clone());

        // Act / Assert: 読めないだけで、落ちない
        assert!(store.load().is_none());

        let _ = fs::remove_dir_all(path.parent().unwrap());
    }
}
