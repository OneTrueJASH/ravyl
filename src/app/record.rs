//! The game record: every move made in this game, ours and the
//! opponent's, kept as a GCG [`Game`] so it can be copied out (`G`) and so
//! a pasted GCG file can be read back in.

use ravyn::board::Board;
use ravyn::gcg::{Action, Game, Placement, Player};
use ravyn::solver::Move;
use ravyn::tile::Tile;

use crate::games::SavedRecord;

/// Nickname for us in the record.
const US: &str = "me";
/// Nickname for the opponent.
const THEM: &str = "opp";

/// The moves of the open game.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Record {
    pub game: Game,
    /// Which GCG player is us: whoever moved first is player 1.
    pub us: usize,
    /// The record began from a position that already had tiles on the
    /// board (a pasted or older saved position), so it can't rebuild the
    /// game from the start.
    pub partial: bool,
}

impl Record {
    /// An empty record for a game at `board`: partial if tiles are already
    /// down.
    pub fn starting_at(board: &Board) -> Self {
        let tiles_down = (0..ravyn::board::BOARD_SIZE * ravyn::board::BOARD_SIZE)
            .any(|i| board.get_tile(i / 15, i % 15).is_some());
        Record {
            partial: tiles_down,
            ..Record::default()
        }
    }

    pub fn is_empty(&self) -> bool {
        self.game.events.is_empty()
    }

    /// The GCG player for our side (`ours`) or theirs; the first move
    /// recorded decides who is player 1.
    fn side(&mut self, ours: bool) -> usize {
        if self.game.events.is_empty() {
            self.us = if ours { 0 } else { 1 };
            let me = Player {
                nick: US.into(),
                name: "Me".into(),
            };
            let them = Player {
                nick: THEM.into(),
                name: "Opponent".into(),
            };
            self.game.players = if ours { [me, them] } else { [them, me] };
        }
        if ours {
            self.us
        } else {
            1 - self.us
        }
    }

    /// Record a move we made from `rack`.
    pub fn ours(&mut self, rack: &[Tile], mv: &Move) {
        let side = self.side(true);
        self.game.push_move(side, rack, mv);
    }

    /// Record the opponent's play: `placed` on `before`, scoring `score`.
    pub fn their_play(&mut self, before: &Board, placed: &[(usize, usize, Tile)], score: u32) {
        let Some(placement) = Placement::from_tiles(before, placed) else {
            return;
        };
        let side = self.side(false);
        self.game
            .push(side, "", Action::Play(placement), score as i32);
    }

    /// Record that the opponent put nothing down: a pass or an exchange,
    /// which the board can't tell apart.
    pub fn their_pass(&mut self) {
        let side = self.side(false);
        self.game
            .push(side, "", Action::Pass, 0)
            .notes
            .push("passed or exchanged".into());
    }

    /// The record as GCG, the opponent named `opponent` when given.
    pub fn to_gcg(&self, opponent: Option<&str>) -> String {
        let mut game = self.game.clone();
        if let Some(name) = opponent {
            game.players[1 - self.us].name = name.to_string();
        }
        if self.partial {
            game.notes.insert(
                0,
                "Recorded from a position with tiles already on the board: \
                 the moves before it are missing."
                    .into(),
            );
        }
        game.to_gcg()
    }

    pub fn saved(&self) -> Option<SavedRecord> {
        (!self.is_empty() || self.partial).then(|| SavedRecord {
            gcg: self.game.to_gcg(),
            us: self.us,
            partial: self.partial,
        })
    }

    pub fn from_saved(saved: &SavedRecord) -> Result<Self, String> {
        let game = if saved.gcg.trim().is_empty() {
            Game::default()
        } else {
            let mut game = Game::parse(&saved.gcg).map_err(|e| e.to_string())?;
            // Lines of the saved text mean nothing once it is loaded.
            game.events.iter_mut().for_each(|e| e.line = 0);
            game
        };
        Ok(Record {
            game,
            us: saved.us.min(1),
            partial: saved.partial,
        })
    }
}

/// A GCG game read for import: the record, the final board, and the
/// scores with ours first.
pub struct Imported {
    pub record: Record,
    pub board: Board,
    pub scores: (u32, u32),
}

/// True if `text` looks like a GCG file rather than a CGP line.
pub fn looks_like_gcg(text: &str) -> bool {
    text.lines()
        .map(str::trim_start)
        .any(|l| l.starts_with('>') || l.starts_with("#player"))
}

/// Read a GCG game to continue it here: the side to move after its last
/// move becomes us.
pub fn import(text: &str) -> Result<Imported, String> {
    let game = Game::parse(text).map_err(|e| e.to_string())?;
    let replay = game
        .replay(&ravyn::rules::CROSSPLAY)
        .map_err(|e| e.to_string())?;
    let last_mover = game.events.iter().rev().find_map(|e| {
        matches!(
            e.action,
            Action::Play(_)
                | Action::Exchange(_)
                | Action::ExchangeCount(_)
                | Action::Pass
                | Action::Withdrawn
        )
        .then_some(e.player)
    });
    let us = last_mover.map_or(0, |p| 1 - p);
    let totals = game.scores_before(game.events.len());
    let score = |p: usize| totals[p].max(0) as u32;
    Ok(Imported {
        scores: (score(us), score(1 - us)),
        board: replay.boards.last().cloned().expect("a final board"),
        record: Record {
            game,
            us,
            partial: false,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use ravyn::solver::Move;

    const GAME: &str = "\
#player1 alice Alice
#player2 sam Sam
>alice: AEINRST 8D NASTIER +58 58
>sam: DEHLOOW H4 HOWD.E +36 36
";

    #[test]
    fn import_makes_the_side_to_move_us() {
        let imported = import(GAME).unwrap();
        assert_eq!(imported.record.us, 0, "alice moves next");
        assert_eq!(imported.scores, (58, 36));
        assert_eq!(imported.board.get_tile(3, 7).unwrap().letter, 'H');
        assert!(looks_like_gcg(GAME));
        assert!(!looks_like_gcg("15/15/15 AB/ 0/0 0"));
    }

    #[test]
    fn recording_and_saving() {
        let mut record = Record::starting_at(&Board::new());
        assert!(!record.partial);
        // They move first: they become player 1.
        let before = Board::new();
        let placed = [(7, 7, Tile::new('A')), (7, 8, Tile::new('X'))];
        record.their_play(&before, &placed, 18);
        record.ours(&[Tile::new('Q')], &Move::pass(&[Tile::new('Q')]));
        record.their_pass();
        assert_eq!(record.us, 1);
        let text = record.to_gcg(Some("Sam"));
        assert!(text.contains("#player1 opp Sam"), "{text}");
        assert!(text.contains(">opp: 8H AX +18 18"), "{text}");
        assert!(text.contains(">me: Q - +0 0"), "{text}");
        assert!(text.contains("#note passed or exchanged"), "{text}");

        let back = Record::from_saved(&record.saved().unwrap()).unwrap();
        assert_eq!(back, record);
    }
}
