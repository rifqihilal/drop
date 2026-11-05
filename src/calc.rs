//! Arithmetic typed into the task field after "=": + - * / and the
//! everyday x and : for times and divided by, % of a hundred, ^ for powers,
//! brackets, and decimals with a point or a comma.

/// The value of `text`, or None when it is not a whole, finite sum.
pub fn evaluate(text: &str) -> Option<f64> {
    let tokens = tokens(text)?;
    let mut parser = Parser { tokens, at: 0 };
    let value = parser.sum()?;
    (parser.at == parser.tokens.len() && value.is_finite()).then_some(value)
}

/// The result written back: at most six decimals, trailing zeros dropped, and
/// a comma for the decimal point when the sum used one.
pub fn format(value: f64, comma: bool) -> String {
    let text = format!("{value:.6}");
    let text = text.trim_end_matches('0').trim_end_matches('.');
    let text = if text == "-0" { "0" } else { text };
    if comma {
        text.replace('.', ",")
    } else {
        text.to_string()
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Token {
    Number(f64),
    Op(char),
    Open,
    Close,
}

fn tokens(text: &str) -> Option<Vec<Token>> {
    let mut tokens = Vec::new();
    let mut chars = text.chars().peekable();
    while let Some(&c) = chars.peek() {
        match c {
            ' ' => {
                chars.next();
            }
            '0'..='9' | '.' | ',' => {
                let mut number = String::new();
                while let Some(&d) = chars.peek() {
                    match d {
                        '0'..='9' => number.push(d),
                        '.' | ',' => number.push('.'),
                        _ => break,
                    }
                    chars.next();
                }
                tokens.push(Token::Number(number.parse().ok()?));
            }
            '+' | '-' | '*' | '/' | '^' | '%' => {
                tokens.push(Token::Op(c));
                chars.next();
            }
            'x' | 'X' | '×' => {
                tokens.push(Token::Op('*'));
                chars.next();
            }
            ':' | '÷' => {
                tokens.push(Token::Op('/'));
                chars.next();
            }
            '(' => {
                tokens.push(Token::Open);
                chars.next();
            }
            ')' => {
                tokens.push(Token::Close);
                chars.next();
            }
            _ => return None,
        }
    }
    Some(tokens)
}

struct Parser {
    tokens: Vec<Token>,
    at: usize,
}

impl Parser {
    fn peek(&self) -> Option<Token> {
        self.tokens.get(self.at).copied()
    }

    fn take(&mut self) -> Option<Token> {
        let token = self.peek();
        self.at += 1;
        token
    }

    /// Terms joined by + and -.
    fn sum(&mut self) -> Option<f64> {
        let mut value = self.product()?;
        while let Some(Token::Op(op @ ('+' | '-'))) = self.peek() {
            self.at += 1;
            let right = self.product()?;
            value = if op == '+' {
                value + right
            } else {
                value - right
            };
        }
        Some(value)
    }

    /// Factors joined by * and /.
    fn product(&mut self) -> Option<f64> {
        let mut value = self.power()?;
        while let Some(Token::Op(op @ ('*' | '/'))) = self.peek() {
            self.at += 1;
            let right = self.power()?;
            value = if op == '*' {
                value * right
            } else {
                value / right
            };
        }
        Some(value)
    }

    /// A number raised by ^, which groups from the right.
    fn power(&mut self) -> Option<f64> {
        let base = self.unary()?;
        if let Some(Token::Op('^')) = self.peek() {
            self.at += 1;
            return Some(base.powf(self.power()?));
        }
        Some(base)
    }

    /// A signed number, bracket or percentage.
    fn unary(&mut self) -> Option<f64> {
        let value = match self.take()? {
            Token::Op('-') => -self.unary()?,
            Token::Op('+') => self.unary()?,
            Token::Number(number) => number,
            Token::Open => {
                let value = self.sum()?;
                (self.take()? == Token::Close).then_some(value)?
            }
            _ => return None,
        };
        if let Some(Token::Op('%')) = self.peek() {
            self.at += 1;
            return Some(value / 100.0);
        }
        Some(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn follows_the_usual_order() {
        assert_eq!(evaluate("12*3+5"), Some(41.0));
        assert_eq!(evaluate("2 + 3 x 4"), Some(14.0));
        assert_eq!(evaluate("(2+3)*4"), Some(20.0));
        assert_eq!(evaluate("2^3^2"), Some(512.0));
        assert_eq!(evaluate("-4 + 10"), Some(6.0));
        assert_eq!(evaluate("150000 * 10%"), Some(15000.0));
        assert_eq!(evaluate("10 : 4"), Some(2.5));
    }

    #[test]
    fn takes_a_decimal_comma() {
        assert_eq!(evaluate("2,5 * 2"), Some(5.0));
        assert_eq!(format(1.25, true), "1,25");
        assert_eq!(format(41.0, false), "41");
        assert_eq!(format(1.0 / 3.0, false), "0.333333");
    }

    #[test]
    fn refuses_what_is_not_a_sum() {
        assert_eq!(evaluate("beli susu"), None);
        assert_eq!(evaluate("2 +"), None);
        assert_eq!(evaluate("(2"), None);
        assert_eq!(evaluate("1/0"), None);
        assert_eq!(evaluate(""), None);
    }
}
