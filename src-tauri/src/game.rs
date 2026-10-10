#![allow(dead_code)]

use std::fmt::Display;
use shakmaty::{
    fen::Fen,
    uci::UciMove as ShakmatyUciMove,
    zobrist::{Zobrist64, ZobristHash},
    CastlingMode, Chess, Color, EnPassantMode, Move, Position, Square,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IllegalMove(pub String);

impl Display for IllegalMove {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "illegal move: {}", self.0)
    }
}

impl std::error::Error for IllegalMove {}

#[derive(Debug, Clone, PartialEq)]
pub struct PlayedMove {
    pub mv: Move,
    pub san: String,
    pub score_cp: Option<i32>,
    pub mate: Option<i32>,
    pub depth: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GameEnd {
    pub result: &'static str,
    pub reason: String,
}

#[derive(Clone, Debug)]
pub struct Game {
    start_pos: Chess,
    start_fen: String,
    current_pos: Chess,
    played_moves: Vec<PlayedMove>,
    rep_history: Vec<Zobrist64>,
    forfeit_end: Option<GameEnd>,
}

const STARTING_FEN: &str = "rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR w KQkq - 0 1";

impl Game {
    pub fn new(start_fen: Option<&str>) -> Result<Self, String> {
        let fen_str = start_fen.unwrap_or(STARTING_FEN);
        let fen: Fen = fen_str
            .parse()
            .map_err(|e| format!("invalid FEN '{}': {}", fen_str, e))?;
        let pos: Chess = fen
            .into_position(CastlingMode::Standard)
            .map_err(|e| format!("illegal position for FEN '{}': {}", fen_str, e))?;

        let initial_hash = pos.zobrist_hash::<Zobrist64>(EnPassantMode::Legal);

        Ok(Self {
            start_pos: pos.clone(),
            start_fen: fen_str.to_string(),
            current_pos: pos,
            played_moves: Vec::new(),
            rep_history: vec![initial_hash],
            forfeit_end: None,
        })
    }

    pub fn play_uci<U: TryIntoUci>(&mut self, uci: U) -> Result<PlayedMove, IllegalMove> {
        let uci_str = uci.try_into_uci_string()?;
        let shakmaty_uci: ShakmatyUciMove = ShakmatyUciMove::from_ascii(uci_str.as_bytes())
            .map_err(|_| IllegalMove(format!("failed to parse UCI move '{}'", uci_str)))?;

        let m = shakmaty_uci
            .to_move(&self.current_pos)
            .map_err(|_| IllegalMove(format!("move '{}' is illegal in current position", uci_str)))?;

        let is_irreversible = self.current_pos.is_irreversible(m.clone());
        let mut next_pos = self.current_pos.clone();
        let san = shakmaty::san::SanPlus::from_move_and_play_unchecked(&mut next_pos, m.clone()).to_string();

        self.current_pos = next_pos;

        let played = PlayedMove {
            mv: m,
            san,
            score_cp: None,
            mate: None,
            depth: None,
        };
        self.played_moves.push(played.clone());

        if is_irreversible {
            self.rep_history.clear();
        }
        let hash = self.current_pos.zobrist_hash::<Zobrist64>(EnPassantMode::Legal);
        self.rep_history.push(hash);

        Ok(played)
    }

    pub fn status(&self) -> Option<GameEnd> {
        if let Some(forfeit) = &self.forfeit_end {
            return Some(forfeit.clone());
        }

        if self.current_pos.is_checkmate() {
            let winner = !self.current_pos.turn();
            let result = match winner {
                Color::White => "1-0",
                Color::Black => "0-1",
            };
            return Some(GameEnd {
                result,
                reason: "Checkmate".to_string(),
            });
        }

        if self.current_pos.is_stalemate() {
            return Some(GameEnd {
                result: "1/2-1/2",
                reason: "Stalemate".to_string(),
            });
        }

        if self.current_pos.is_insufficient_material() {
            return Some(GameEnd {
                result: "1/2-1/2",
                reason: "Insufficient material".to_string(),
            });
        }

        if self.current_pos.halfmoves() >= 100 {
            return Some(GameEnd {
                result: "1/2-1/2",
                reason: "50-move rule".to_string(),
            });
        }

        let curr_hash = self.current_pos.zobrist_hash::<Zobrist64>(EnPassantMode::Legal);
        let repetitions = self.rep_history.iter().filter(|&&h| h == curr_hash).count();
        if repetitions >= 3 {
            return Some(GameEnd {
                result: "1/2-1/2",
                reason: "Threefold repetition".to_string(),
            });
        }

        None
    }

    pub fn forfeit(&mut self, loser: Color, reason: &str) -> GameEnd {
        let result = match loser {
            Color::White => "0-1",
            Color::Black => "1-0",
        };
        let end = GameEnd {
            result,
            reason: reason.to_string(),
        };
        self.forfeit_end = Some(end.clone());
        end
    }

    pub fn pv_to_san<U: TryIntoUci>(&self, pv: &[U]) -> (Vec<String>, String) {
        let mut sim_pos = self.current_pos.clone();
        let mut san_list = Vec::with_capacity(pv.len());

        for u in pv {
            let uci_str = match u.try_into_uci_string() {
                Ok(s) => s,
                Err(_) => break,
            };
            let shakmaty_uci = match ShakmatyUciMove::from_ascii(uci_str.as_bytes()) {
                Ok(m) => m,
                Err(_) => break,
            };
            let m = match shakmaty_uci.to_move(&sim_pos) {
                Ok(m) => m,
                Err(_) => break,
            };
            let mut next_pos = sim_pos.clone();
            let san = shakmaty::san::SanPlus::from_move_and_play_unchecked(&mut next_pos, m).to_string();
            sim_pos = next_pos;
            san_list.push(san);
        }

        let end_fen = Fen::from_position(&sim_pos, EnPassantMode::Legal).to_string();
        (san_list, end_fen)
    }

    pub fn fen(&self) -> String {
        Fen::from_position(&self.current_pos, EnPassantMode::Legal).to_string()
    }

    pub fn to_fen(&self) -> String {
        self.fen()
    }
    pub fn start_fen(&self) -> String {
        self.start_fen.clone()
    }

    pub fn is_custom_fen(&self) -> bool {
        let default_fen = Fen::from_position(&Chess::default(), EnPassantMode::Legal).to_string();
        self.start_fen != default_fen
    }
    pub fn last_move_squares(&self) -> Option<(Square, Square)> {
        self.played_moves.last().map(|pm| (pm.mv.from().unwrap_or(Square::A1), pm.mv.to()))
    }

    pub fn san_history(&self) -> Vec<String> {
        self.played_moves.iter().map(|pm| pm.san.clone()).collect()
    }

    pub fn is_check(&self) -> bool {
        self.current_pos.is_check()
    }

    pub fn side_to_move(&self) -> Color {
        self.current_pos.turn()
    }

    pub fn current_pos(&self) -> &Chess {
        &self.current_pos
    }

    pub fn played_moves(&self) -> &[PlayedMove] {
        &self.played_moves
    }

    pub fn played_moves_mut(&mut self) -> &mut [PlayedMove] {
        &mut self.played_moves
    }

    pub fn set_last_move_eval(&mut self, score_cp: Option<i32>, mate: Option<i32>, depth: Option<u32>) {
        if let Some(pm) = self.played_moves.last_mut() {
            pm.score_cp = score_cp;
            pm.mate = mate;
            pm.depth = depth;
        }
    }

    pub fn to_pgn(&self, white_name: &str, black_name: &str) -> String {
        let game_status = self.status();
        let result = game_status
            .as_ref()
            .map(|s| s.result)
            .unwrap_or("*");

        let mut out = String::new();
        out.push_str("[Event \"Chess Battle Arena\"]\n");
        out.push_str("[Site \"Local\"]\n");
        out.push_str("[Date \"????.??.??\"]\n");
        out.push_str("[Round \"1\"]\n");
        out.push_str(&format!("[White \"{}\"]\n", white_name));
        out.push_str(&format!("[Black \"{}\"]\n", black_name));
        out.push_str(&format!("[Result \"{}\"]\n", result));

        let default_fen = Fen::from_position(&Chess::default(), EnPassantMode::Legal).to_string();
        let is_custom_fen = self.start_fen != default_fen;

        if is_custom_fen {
            out.push_str("[SetUp \"1\"]\n");
            out.push_str(&format!("[FEN \"{}\"]\n", self.start_fen));
        }

        if let Some(status) = &game_status {
            out.push_str(&format!("[Termination \"{}\"]\n", status.reason));
        }
        out.push_str(&format!("[PlyCount \"{}\"]\n", self.played_moves.len()));
        out.push('\n');

        let mut tokens: Vec<String> = Vec::new();
        let start_turn = self.start_pos.turn();
        let start_fullmove = self.start_pos.fullmoves().get();

        let mut turn = start_turn;
        let mut move_num = start_fullmove;

        for (idx, pm) in self.played_moves.iter().enumerate() {
            let move_prefix = if turn == Color::White {
                format!("{}. {}", move_num, pm.san)
            } else if idx == 0 && start_turn == Color::Black {
                format!("{}... {}", move_num, pm.san)
            } else {
                pm.san.clone()
            };

            let eval_comment = match (pm.mate, pm.score_cp, pm.depth) {
                (Some(m), _, Some(d)) => {
                    let sign = if m > 0 { "#" } else { "#-" };
                    Some(format!("{{{}{}/{}}}", sign, m.abs(), d))
                }
                (Some(m), _, None) => {
                    let sign = if m > 0 { "#" } else { "#-" };
                    Some(format!("{{{}{}}}", sign, m.abs()))
                }
                (None, Some(cp), Some(d)) => {
                    let cp_val = cp as f64 / 100.0;
                    Some(format!("{{{:+.2}/{}}}", cp_val, d))
                }
                (None, Some(cp), None) => {
                    let cp_val = cp as f64 / 100.0;
                    Some(format!("{{{:+.2}}}", cp_val))
                }
                _ => None,
            };

            tokens.push(move_prefix);
            if let Some(comment) = eval_comment {
                tokens.push(comment);
            }

            if turn == Color::Black {
                move_num += 1;
                turn = Color::White;
            } else {
                turn = Color::Black;
            }
        }

        tokens.push(result.to_string());

        let mut lines = Vec::new();
        let mut cur_line = String::new();
        for token in tokens {
            if cur_line.is_empty() {
                cur_line.push_str(&token);
            } else if cur_line.len() + 1 + token.len() <= 80 {
                cur_line.push(' ');
                cur_line.push_str(&token);
            } else {
                lines.push(cur_line);
                cur_line = token;
            }
        }
        if !cur_line.is_empty() {
            lines.push(cur_line);
        }

        out.push_str(&lines.join("\n"));
        out.push('\n');

        out
    }
}

pub trait TryIntoUci {
    fn try_into_uci_string(&self) -> Result<String, IllegalMove>;
}

impl TryIntoUci for &str {
    fn try_into_uci_string(&self) -> Result<String, IllegalMove> {
        Ok(self.to_string())
    }
}

impl TryIntoUci for String {
    fn try_into_uci_string(&self) -> Result<String, IllegalMove> {
        Ok(self.clone())
    }
}

impl TryIntoUci for vampirc_uci::UciMove {
    fn try_into_uci_string(&self) -> Result<String, IllegalMove> {
        Ok(self.to_string())
    }
}

impl TryIntoUci for ShakmatyUciMove {
    fn try_into_uci_string(&self) -> Result<String, IllegalMove> {
        Ok(self.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use shakmaty::Square;

    #[test]
    fn test_illegal_move_rejected() {
        let mut game = Game::new(None).unwrap();
        let res = game.play_uci("e2e5");
        assert!(res.is_err());
        assert_eq!(game.played_moves().len(), 0);
    }

    #[test]
    fn test_checkmate() {
        // Scholar's Mate
        // 1. e4 e5 2. Qh5 Nc6 3. Bc4 Nf6 4. Qxf7#
        let mut game = Game::new(None).unwrap();
        assert_eq!(game.status(), None);

        game.play_uci("e2e4").unwrap();
        game.play_uci("e7e5").unwrap();
        game.play_uci("d1h5").unwrap();
        game.play_uci("b8c6").unwrap();
        game.play_uci("f1c4").unwrap();
        game.play_uci("g8f6").unwrap();
        let last = game.play_uci("h5f7").unwrap();
        assert_eq!(last.san, "Qxf7#");

        assert!(game.is_check());
        let status = game.status();
        assert_eq!(
            status,
            Some(GameEnd {
                result: "1-0",
                reason: "Checkmate".to_string(),
            })
        );
    }

    #[test]
    fn test_stalemate() {
        // FEN with Black king on a8, White king on c7, White queen on c6 -> play Qc7 stalemate?
        // Let's use a known stalemate fen: 7k/5Q2/6K1/8/8/8/8/8 b - - 0 1
        let fen = "7k/5Q2/6K1/8/8/8/8/8 b - - 0 1";
        let game = Game::new(Some(fen)).unwrap();
        assert_eq!(
            game.status(),
            Some(GameEnd {
                result: "1/2-1/2",
                reason: "Stalemate".to_string(),
            })
        );
    }

    #[test]
    fn test_insufficient_material() {
        // King vs King
        let fen = "8/8/8/4k3/8/8/4K3/8 w - - 0 1";
        let game = Game::new(Some(fen)).unwrap();
        assert_eq!(
            game.status(),
            Some(GameEnd {
                result: "1/2-1/2",
                reason: "Insufficient material".to_string(),
            })
        );
    }

    #[test]
    fn test_50_move_rule() {
        // Position with 99 halfmoves, make 1 non-zeroing move -> 100 halfmoves -> draw
        // 8/8/8/4k3/8/8/4K3/R7 w - - 99 50
        // Wait, Ra1 to Ra2 is quiet move with no pawn / no capture
        let fen = "8/8/8/4k3/8/8/4K3/R7 w - - 99 50";
        let mut game = Game::new(Some(fen)).unwrap();
        assert_eq!(game.status(), None);
        game.play_uci("a1a2").unwrap();
        assert_eq!(
            game.status(),
            Some(GameEnd {
                result: "1/2-1/2",
                reason: "50-move rule".to_string(),
            })
        );
    }

    #[test]
    fn test_threefold_repetition() {
        let mut game = Game::new(None).unwrap();
        // 1. Nf3 Nf6 2. Ng1 Ng8 (1st repetition)
        // 3. Nf3 Nf6 4. Ng1 Ng8 (2nd repetition -> 3 occurrences of start position)
        game.play_uci("g1f3").unwrap();
        game.play_uci("g8f6").unwrap();
        game.play_uci("f3g1").unwrap();
        game.play_uci("f6g8").unwrap();
        assert_eq!(game.status(), None);

        game.play_uci("g1f3").unwrap();
        game.play_uci("g8f6").unwrap();
        game.play_uci("f3g1").unwrap();
        game.play_uci("f6g8").unwrap();
        assert_eq!(
            game.status(),
            Some(GameEnd {
                result: "1/2-1/2",
                reason: "Threefold repetition".to_string(),
            })
        );
    }

    #[test]
    fn test_custom_fen_pgn_tags() {
        let fen = "rnbqkbnr/pp1ppppp/8/2p5/4P3/8/PPPP1PPP/RNBQKBNR w KQkq c6 0 2";
        let mut game = Game::new(Some(fen)).unwrap();
        game.play_uci("g1f3").unwrap();
        let pgn = game.to_pgn("Stockfish", "Lc0");
        assert!(pgn.contains("[SetUp \"1\"]\n"));
        assert!(pgn.contains(&format!("[FEN \"{}\"]\n", fen)));
        assert!(pgn.contains("[White \"Stockfish\"]\n"));
        assert!(pgn.contains("[Black \"Lc0\"]\n"));
        assert!(pgn.contains("[Round \"1\"]\n"));
    }

    #[test]
    fn test_standard_fen_no_setup_tag() {
        let mut game = Game::new(None).unwrap();
        game.play_uci("e2e4").unwrap();
        let pgn = game.to_pgn("SF", "GNU");
        assert!(!pgn.contains("[SetUp"));
        assert!(!pgn.contains("[FEN"));
    }

    #[test]
    fn test_pv_truncation() {
        let game = Game::new(None).unwrap();
        // PV: e2e4, e7e5, illegal move e2e5
        let pv = vec!["e2e4", "e7e5", "e2e5", "g8f6"];
        let (san, end_fen) = game.pv_to_san(&pv);
        assert_eq!(san, vec!["e4", "e5"]);
        assert!(end_fen.contains("rnbqkbnr/pppp1ppp/8/4p3/4P3/8/PPPP1PPP/RNBQKBNR w KQkq - 0 2"));
    }

    #[test]
    fn test_forfeit_pgn() {
        let mut game = Game::new(None).unwrap();
        game.play_uci("e2e4").unwrap();
        game.forfeit(Color::Black, "Illegal bestmove: e7e4");
        let status = game.status();
        assert_eq!(
            status,
            Some(GameEnd {
                result: "1-0",
                reason: "Illegal bestmove: e7e4".to_string(),
            })
        );
        let pgn = game.to_pgn("WhiteEngine", "BlackEngine");
        assert!(pgn.contains("[Result \"1-0\"]"));
        assert!(pgn.contains("[Termination \"Illegal bestmove: e7e4\"]"));
        assert!(pgn.ends_with("1-0\n"));
    }

    #[test]
    fn test_pgn_eval_comments_and_wrapping() {
        let mut game = Game::new(None).unwrap();
        game.play_uci("e2e4").unwrap();
        game.set_last_move_eval(Some(35), None, Some(22));
        game.play_uci("e7e5").unwrap();
        game.set_last_move_eval(None, Some(5), Some(18));
        let pgn = game.to_pgn("White", "Black");
        assert!(pgn.contains("1. e4 {+0.35/22} e5 {#5/18} *"));
    }

    #[test]
    fn test_last_move_squares() {
        let mut game = Game::new(None).unwrap();
        assert_eq!(game.last_move_squares(), None);
        game.play_uci("e2e4").unwrap();
        assert_eq!(game.last_move_squares(), Some((Square::E2, Square::E4)));
    }
}
