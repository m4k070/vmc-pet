//! M5Stack の時計を PC の時計に合わせるための、USB シリアルで受け取る行の解釈。
//!
//! # なぜ必要か
//!
//! M5Stack には OS も NTP も無く、時刻の出どころは電池で動き続ける RTC(BM8563)
//! だけである。バックアップが尽きて時刻を失うと、clock.rs が固定の基準時刻
//! (2020-01-01)から数え直す。停止していた時間を測るだけならそれで足りたが、
//! 世話の予測(`care_prediction`)は **1日のうちの何時か** を学ぶので、時計が
//! 実際の時刻とずれていると、PC と M5Stack で同じ時刻が別の「何時」になる。
//!
//! そこで PC の時計(UTC の Unix 時刻。PC版が `now_unix_seconds` で読んでいるのと
//! 同じもの)を、USB シリアルで1行送って合わせられるようにする。WiFi と NTP を
//! 足すより依存が小さく、書き込みと同じケーブルで済む。
//!
//! # 形式
//!
//! ```text
//! time <Unix 秒>\n
//! ```
//!
//! 行末は `\n`・`\r`・`\r\n` のどれでもよい(端末やシェルで改行の扱いが違うため)。
//! 知らない行は捨てて、そう返す。
//!
//! ここに置くのは、M5Stack 側ではテストが動かせないため(`civil_time` と同じ理由)。

/// 1行に受け取る最大の長さ。`time ` と u64 の最大桁数(20桁)が収まる。
const MAX_LINE_LEN: usize = 32;

/// 受け取った1行の意味。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SerialCommand {
    /// 時計をこの Unix 時刻(秒、UTC)に合わせる。
    SetTime { unix_seconds: u64 },
    /// 形式に合わない行。
    Unrecognized,
}

/// シリアルから1バイトずつ受け取り、行がそろったら解釈する。
///
/// 受信は描画の合間に少しずつ読むので、1行が何回かに分かれて届く。途中までを
/// ここに溜めておく。
#[derive(Debug, Clone)]
pub struct LineReader {
    line: [u8; MAX_LINE_LEN],
    len: usize,
    /// いまの行が `MAX_LINE_LEN` を超えたか。超えた行は、行末まで読み捨てて
    /// `Unrecognized` とする(途中で切った値を時刻として使わないため)。
    overflowed: bool,
}

impl LineReader {
    pub const fn new() -> Self {
        Self {
            line: [0; MAX_LINE_LEN],
            len: 0,
            overflowed: false,
        }
    }

    /// 1バイト受け取る。行が終わったら、その行の解釈を返す。空行は何も返さない。
    pub fn push(&mut self, byte: u8) -> Option<SerialCommand> {
        let is_line_end = byte == b'\n' || byte == b'\r';
        if !is_line_end {
            if self.len < MAX_LINE_LEN {
                self.line[self.len] = byte;
                self.len += 1;
            } else {
                self.overflowed = true;
            }
            return None;
        }

        let is_empty_line = self.len == 0 && !self.overflowed;
        if is_empty_line {
            // `\r\n` の `\n` もここに来る
            return None;
        }
        let command = if self.overflowed {
            SerialCommand::Unrecognized
        } else {
            parse_line(&self.line[..self.len])
        };
        self.len = 0;
        self.overflowed = false;
        Some(command)
    }
}

impl Default for LineReader {
    fn default() -> Self {
        Self::new()
    }
}

/// 1行(行末を除く)を解釈する。
fn parse_line(line: &[u8]) -> SerialCommand {
    let Ok(text) = core::str::from_utf8(line) else {
        return SerialCommand::Unrecognized;
    };
    let mut words = text.split_ascii_whitespace();
    let (Some("time"), Some(value), None) = (words.next(), words.next(), words.next()) else {
        return SerialCommand::Unrecognized;
    };
    match value.parse::<u64>() {
        Ok(unix_seconds) => SerialCommand::SetTime { unix_seconds },
        Err(_) => SerialCommand::Unrecognized,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// バイト列をすべて流し込み、返ってきた解釈を順に集める。
    fn feed(reader: &mut LineReader, bytes: &[u8]) -> Vec<SerialCommand> {
        bytes.iter().filter_map(|&byte| reader.push(byte)).collect()
    }

    #[test]
    fn a_time_line_sets_the_time() {
        // Arrange
        let mut reader = LineReader::new();

        // Act
        let commands = feed(&mut reader, b"time 1789302896\n");

        // Assert
        assert_eq!(
            commands,
            [SerialCommand::SetTime {
                unix_seconds: 1_789_302_896
            }]
        );
    }

    #[test]
    fn crlf_line_endings_give_one_command_not_two() {
        // Arrange
        let mut reader = LineReader::new();

        // Act
        let commands = feed(&mut reader, b"time 42\r\n");

        // Assert
        assert_eq!(commands, [SerialCommand::SetTime { unix_seconds: 42 }]);
    }

    #[test]
    fn a_line_split_across_reads_is_reassembled() {
        // Arrange: 描画の合間に少しずつ読むので、1行が分かれて届く
        let mut reader = LineReader::new();

        // Act
        let first = feed(&mut reader, b"tim");
        let second = feed(&mut reader, b"e 7");
        let third = feed(&mut reader, b"\n");

        // Assert
        assert!(first.is_empty() && second.is_empty());
        assert_eq!(third, [SerialCommand::SetTime { unix_seconds: 7 }]);
    }

    #[test]
    fn empty_lines_are_ignored() {
        // Arrange
        let mut reader = LineReader::new();

        // Act / Assert
        assert!(feed(&mut reader, b"\n\r\n\r").is_empty());
    }

    #[test]
    fn malformed_lines_are_unrecognized() {
        // Arrange
        let malformed: [&[u8]; 6] = [
            b"hello\n",
            b"time\n",
            b"time -5\n",
            b"time 12x\n",
            b"time 1 2\n",
            b"time \xff\n",
        ];

        // Act / Assert
        for line in malformed {
            let mut reader = LineReader::new();
            assert_eq!(
                feed(&mut reader, line),
                [SerialCommand::Unrecognized],
                "{line:?} must not be taken as a time"
            );
        }
    }

    #[test]
    fn an_overlong_line_is_rejected_as_a_whole_and_the_next_line_still_works() {
        // Arrange: 途中で切った数字を時刻として使ってはいけない
        let mut reader = LineReader::new();
        let mut overlong = b"time ".to_vec();
        overlong.extend(core::iter::repeat_n(b'1', 40));
        overlong.push(b'\n');

        // Act
        let rejected = feed(&mut reader, &overlong);
        let accepted = feed(&mut reader, b"time 9\n");

        // Assert
        assert_eq!(rejected, [SerialCommand::Unrecognized]);
        assert_eq!(accepted, [SerialCommand::SetTime { unix_seconds: 9 }]);
    }

    #[test]
    fn surrounding_spaces_are_tolerated() {
        // Arrange
        let mut reader = LineReader::new();

        // Act / Assert
        assert_eq!(
            feed(&mut reader, b"  time   5  \n"),
            [SerialCommand::SetTime { unix_seconds: 5 }]
        );
    }
}
