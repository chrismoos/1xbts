use std::collections::VecDeque;

pub struct SymbolRepetition {
    factor: usize,
    symbols: VecDeque<u8>,
    repeat: usize,
}

impl SymbolRepetition {
    pub fn new(factor: usize) -> SymbolRepetition {
        assert!(factor > 0);
        SymbolRepetition {
            factor,
            symbols: VecDeque::new(),
            repeat: 0,
        }
    }

    pub fn feed(&mut self, symbol: u8) {
        self.symbols.push_back(symbol);
        if self.repeat == 0 {
            self.repeat = self.factor - 1;
        }
    }

    pub fn next(&mut self) -> Option<u8> {
        if self.repeat == 0 {
            if let Some(symbol) = self.symbols.pop_front() {
                self.repeat = self.factor - 1;
                Some(symbol)
            } else {
                None
            }
        } else {
            self.repeat -= 1;
            if let Some(symbol) = self.symbols.front() {
                Some(*symbol)
            } else {
                None
            }
        }
    }

    pub fn take_all(&mut self) -> Vec<u8> {
        let mut output = Vec::new();
        loop {
            if let Some(next) = self.next() {
                output.push(next);
            } else {
                break;
            }
        }
        output
    }
}

#[cfg(test)]
mod tests {
    use super::SymbolRepetition;

    #[test]
    pub fn test_symbol_repeat_1x() {
        let mut sr = SymbolRepetition::new(1);
        sr.feed(1);
        sr.feed(0);
        sr.feed(1);

        let output = sr.take_all();
        assert_eq!(&[1, 0, 1], &output[..]);
    }

    #[test]
    pub fn test_symbol_repeat_2x() {
        let mut sr = SymbolRepetition::new(2);
        sr.feed(1);
        sr.feed(0);
        sr.feed(1);

        let output = sr.take_all();
        assert_eq!(&[1, 1, 0, 0, 1, 1], &output[..]);
    }

    #[test]
    pub fn test_symbol_repeat_2x_consecutive() {
        let mut sr = SymbolRepetition::new(2);
        sr.feed(1);
        sr.feed(0);
        sr.feed(1);

        let output = sr.take_all();
        assert_eq!(&[1, 1, 0, 0, 1, 1], &output[..]);

        sr.feed(1);
        sr.feed(0);
        sr.feed(1);

        let output = sr.take_all();
        assert_eq!(&[1, 1, 0, 0, 1, 1], &output[..]);
    }
}
