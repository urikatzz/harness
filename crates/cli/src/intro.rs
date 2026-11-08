//! Startup animation: an ASCII crown hops in from the right, lands, and sparkles.

use std::io::{self, IsTerminal, Write};
use std::thread::sleep;
use std::time::Duration;

const GOLD: &str = "\x1b[33m";
const BOLD: &str = "\x1b[1m";
const DIM: &str = "\x1b[2m";
const RESET: &str = "\x1b[0m";
const HIDE_CURSOR: &str = "\x1b[?25l";
const SHOW_CURSOR: &str = "\x1b[?25h";

/// Set this env var to any value to skip the intro.
const SKIP_ENV: &str = "URITHEKING_NO_INTRO";

const CROWN: [&str; 4] = [
    r"   _.+._   ",
    r" (^\/^\/^) ",
    r"  \@*@*@/  ",
    r"  {_____}  ",
];
/// Crown flattened for one frame when it hits the ground.
const SQUASHED: [&str; 4] = [
    r"           ",
    r"  _.-+-._  ",
    r"(^\/^\/^\/^)",
    r" {_______} ",
];
const SPARKLES: [&str; 4] = [r"  *    .  ", r"    +     ", r" .     *  ", r"   *  +   "];

pub struct Intro {
    /// Rows the crown can rise above the ground.
    jump: usize,
    start_col: usize,
    end_col: usize,
    hops: usize,
    frames: usize,
    frame_time: Duration,
}

impl Default for Intro {
    fn default() -> Self {
        Self {
            jump: 5,
            start_col: 40,
            end_col: 2,
            hops: 3,
            frames: 54,
            frame_time: Duration::from_millis(28),
        }
    }
}

/// What a single frame shows.
struct Frame {
    col: usize,
    lift: usize,
    squash: bool,
    sparkle: bool,
}

impl Intro {
    /// Plays only on a real terminal, so piped output and scripts stay clean.
    pub fn play_if_interactive(&self) {
        if !io::stdout().is_terminal() || std::env::var_os(SKIP_ENV).is_some() {
            return;
        }
        let mut out = io::stdout().lock();
        // Animation is cosmetic; on a write error we stop, but always restore the cursor.
        let _ = self.play(&mut out);
        let _ = write!(out, "{SHOW_CURSOR}");
        let _ = out.flush();
    }

    /// Total rows drawn: air above the crown + the crown + the ground line.
    fn height(&self) -> usize {
        self.jump + CROWN.len() + 1
    }

    fn play(&self, out: &mut impl Write) -> io::Result<()> {
        write!(out, "{HIDE_CURSOR}{}", "\n".repeat(self.height()))?;
        for f in 0..=self.frames {
            let (col, lift) = self.position(f);
            let landing = lift == 0 && f > 0 && self.position(f - 1).1 > 0;
            self.draw(out, &Frame { col, lift, squash: landing, sparkle: false })?;
            sleep(self.frame_time);
        }
        for _ in 0..3 {
            self.draw(out, &Frame { col: self.end_col, lift: 0, squash: false, sparkle: true })?;
            sleep(Duration::from_millis(140));
            self.draw(out, &Frame { col: self.end_col, lift: 0, squash: false, sparkle: false })?;
            sleep(Duration::from_millis(90));
        }
        // Wipe the animation area so the banner prints in its place.
        write!(out, "\x1b[{}A\x1b[J", self.height())?;
        out.flush()
    }

    /// Redraws the whole area for one frame.
    fn draw(&self, out: &mut impl Write, frame: &Frame) -> io::Result<()> {
        let sprite = if frame.squash { &SQUASHED } else { &CROWN };
        let top = self.jump - frame.lift;
        let pad = " ".repeat(frame.col);
        write!(out, "\x1b[{}A", self.height())?;
        for row in 0..self.height() {
            write!(out, "\r\x1b[2K")?;
            if let Some(line) = row.checked_sub(top).and_then(|i| sprite.get(i)) {
                write!(out, "{pad}{GOLD}{BOLD}{line}{RESET}")?;
                if frame.sparkle {
                    write!(out, "{GOLD}{}{RESET}", SPARKLES[row - top])?;
                }
            } else if row == self.height() - 1 {
                // Ground with a shadow that shrinks as the crown rises.
                let shadow = 9usize.saturating_sub(frame.lift * 2).max(3);
                let offset = (11 - shadow) / 2;
                write!(out, "{pad}{}{DIM}{}{RESET}", " ".repeat(offset), "▔".repeat(shadow))?;
            }
            writeln!(out)?;
        }
        out.flush()
    }

    /// Crown column and height above ground at `frame`: moves left, bouncing in shrinking arcs.
    fn position(&self, frame: usize) -> (usize, usize) {
        let t = frame as f64 / self.frames as f64; // 0.0 → 1.0
        let col = self.start_col as f64 + (self.end_col as f64 - self.start_col as f64) * t;
        let progress = t * self.hops as f64;
        let hop = (progress as usize).min(self.hops - 1);
        let phase = if t >= 1.0 { 1.0 } else { progress.fract() };
        let peak = self.jump as f64 / (hop + 1) as f64;
        let lift = 4.0 * phase * (1.0 - phase) * peak;
        (col.round() as usize, lift.round() as usize)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn starts_and_ends_on_the_ground() {
        let intro = Intro::default();
        assert_eq!(intro.position(0), (intro.start_col, 0));
        assert_eq!(intro.position(intro.frames), (intro.end_col, 0));
    }

    #[test]
    fn first_hop_is_the_highest() {
        let intro = Intro::default();
        let highest = (0..=intro.frames).map(|f| intro.position(f).1).max();
        assert_eq!(highest, Some(intro.jump));
    }

    #[test]
    fn never_leaves_the_area() {
        let intro = Intro::default();
        assert!((0..=intro.frames).all(|f| intro.position(f).1 <= intro.jump));
    }

    #[test]
    fn crown_sprites_have_matching_rows() {
        assert_eq!(CROWN.len(), SQUASHED.len());
        assert_eq!(CROWN.len(), SPARKLES.len());
    }
}
