use crate::expression::Expr;
use crate::tokeniser::tokenizer::{MultiCharTokens, Token};
use std::io::Write;

pub fn write_expr(expr: &Expr) -> Result<String, WriterError> {
    let mut buf = vec![];
    let mut writer = Writer::new(&mut buf);

    match internal::get_expr_type(expr) {
        internal::ExprType::Text(text) => {
            writer.write_str(text)?;
        }
        internal::ExprType::Code(expr) => {
            writer.write_code_start()?;
            writer.write_expr(expr)?;
            writer.write_code_end()?;
        }
        internal::ExprType::StandAloneVariable() => {
            writer.write_expr(&expr)?;
        }
    }

    Ok(String::from_utf8(buf).unwrap_or_else(|err| panic!("invalid UTF-8: {err:?}")))
}

struct Writer<W> {
    inner: W,
}

#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum WriterError {
    #[error("write failed: {0}")]
    Io(#[from] std::io::Error),
}

impl<W: Write> Writer<W> {
    fn new(w: W) -> Self {
        Self { inner: w }
    }

    fn write_code_start(&mut self) -> Result<(), WriterError> {
        self.write_display(Token::MultiChar(MultiCharTokens::InterpolationStart))
    }

    fn write_code_end(&mut self) -> Result<(), WriterError> {
        self.write_display(Token::RCurly)
    }

    fn write_expr(&mut self, expr: &Expr) -> Result<(), WriterError> {
        match expr {
            Expr::Literal(string) => {
                self.write_display(Token::Quote)?;
                self.write_str(string)?;
                self.write_display(Token::Quote)
            }
            Expr::Request() => self.write_display(Token::MultiChar(MultiCharTokens::Request)),
            Expr::Let(let_variable, expr) => {
                self.write_str("let ")?;
                self.write_str(let_variable)?;
                self.write_str(" = ")?;
                self.write_expr(&*expr)?;
                self.write_display(Token::SemiColon)
            }
            Expr::Worker() => self.write_display(Token::MultiChar(MultiCharTokens::Worker)),
            Expr::SelectField(expr, field_name) => {
                self.write_expr(&*expr)?;
                self.write_str(".")?;
                self.write_str(field_name)
            }
            Expr::SelectIndex(expr, index) => {
                self.write_expr(&*expr)?;
                self.write_display(Token::LSquare)?;
                self.write_display(&*index)?;
                self.write_display(Token::RSquare)
            }
            Expr::Sequence(sequence) => {
                self.write_display(Token::LSquare)?;
                for (idx, expr) in sequence.iter().enumerate() {
                    if idx != 0 {
                        self.write_display(Token::Comma)?;
                        self.write_display(Token::Space)?;
                    }
                    self.write_expr(&*expr)?;
                }
                self.write_display(Token::RSquare)
            }
            Expr::Record(record) => {
                self.write_display(Token::LCurly)?;
                for (idx, (key, value)) in record.iter().enumerate() {
                    if idx != 0 {
                        self.write_display(Token::Comma)?;
                        self.write_display(Token::Space)?;
                    }
                    self.write_str(key)?;
                    self.write_display(Token::Colon)?;
                    self.write_display(Token::Space)?;
                    self.write_expr(&*value)?;
                }
                self.write_display(Token::RCurly)
            }
            Expr::Tuple(tuple) => {
                self.write_display(Token::LParen)?;
                for (idx, expr) in tuple.iter().enumerate() {
                    if idx != 0 {
                        self.write_display(Token::Comma)?;
                        self.write_display(Token::Space)?;
                    }
                    self.write_expr(&*expr)?;
                }
                self.write_display(Token::RParen)
            }
            Expr::Number(number) => self.write_display(number),
            Expr::Flags(flags) => {
                self.write_display(Token::LCurly)?;
                for (idx, flag) in flags.iter().enumerate() {
                    if idx != 0 {
                        self.write_display(Token::Comma)?;
                        self.write_display(Token::Space)?;
                    }
                    self.write_str(flag)?;
                }
                self.write_display(Token::RCurly)
            }
            Expr::Variable(variable) => {
                self.write_display(Token::MultiChar(MultiCharTokens::InterpolationStart))?;
                self.write_str(variable)?;
                self.write_display(Token::RCurly)
            }
            Expr::Boolean(bool) => self.write_display(bool),
            Expr::Concat(concatenated) => {
                for expr in concatenated.iter() {
                    self.write_expr(&*expr)?
                }
                Ok(())
            }
            Expr::Multiple(expr) => {
                for (idx, expr) in expr.iter().enumerate() {
                    if idx != 0 {
                        self.write_display(Token::NewLine)?;
                    }
                    self.write_expr(&*expr)?;
                }
                Ok(())
            }
            Expr::Not(expr) => {
                self.write_str("!")?;
                self.write_expr(&*expr)
            }
            Expr::GreaterThan(left, right) => {
                self.write_expr(&*left)?;
                self.write_str(" > ")?;
                self.write_expr(&*right)
            }
            Expr::GreaterThanOrEqualTo(left, right) => {
                self.write_expr(&*left)?;
                self.write_str(" >= ")?;
                self.write_expr(&*right)
            }
            Expr::LessThanOrEqualTo(left, right) => {
                self.write_expr(&*left)?;
                self.write_str(" <= ")?;
                self.write_expr(&*right)
            }
            Expr::EqualTo(left, right) => {
                self.write_expr(&*left)?;
                self.write_str(" == ")?;
                self.write_expr(&*right)
            }
            Expr::LessThan(left, right) => {
                self.write_expr(&*left)?;
                self.write_str(" < ")?;
                self.write_expr(&*right)
            }
            Expr::Cond(if_expr, left, right) => {
                self.write_str("if ")?;
                self.write_expr(&*if_expr)?;
                self.write_str(" then ")?;
                self.write_expr(&*left)?;
                self.write_str(" else ")?;
                self.write_expr(&*right)
            }
            Expr::PatternMatch(match_expr, match_terms) => {
                self.write_str("match ")?;
                self.write_expr(&*match_expr)?;
                self.write_str(" { ")?;
                self.write_display(Token::NewLine)?;
                for (idx, match_term) in match_terms.iter().enumerate() {
                    if idx != 0 {
                        self.write_display(Token::NewLine)?;
                    }
                    let (match_case, match_expr) = &match_term.0;
                    internal::write_constructor(match_case, self)?;
                    self.write_str(" => ")?;
                    self.write_expr(&*match_expr)?;
                }

                Ok(())
            }
            Expr::Constructor0(constructor) => internal::write_constructor(constructor, self),
        }
    }

    fn write_str(&mut self, s: impl AsRef<str>) -> Result<(), WriterError> {
        self.inner.write_all(s.as_ref().as_bytes())?;
        Ok(())
    }

    fn write_display(&mut self, d: impl std::fmt::Display) -> Result<(), WriterError> {
        write!(self.inner, "{d}")?;
        Ok(())
    }
}

mod internal {
    use crate::expression::writer::{Writer, WriterError};
    use crate::expression::{ConstructorPattern, Expr};

    pub(crate) enum ExprType<'a> {
        Code(&'a Expr),
        Text(&'a str),
        StandAloneVariable(),
    }

    pub(crate) fn get_expr_type(expr: &Expr) -> ExprType {
        match expr {
            Expr::Literal(str) => ExprType::Text(str),
            Expr::Variable(_) => ExprType::StandAloneVariable(),
            expr => ExprType::Code(expr),
        }
    }

    pub(crate) fn write_constructor<W>(
        match_case: &ConstructorPattern,
        writer: &mut Writer<W>,
    ) -> Result<(), WriterError>
    where
        W: std::io::Write,
    {
        match match_case {
            ConstructorPattern::WildCard => writer.write_str("_"),
            ConstructorPattern::As(name, pattern) => {
                writer.write_str(name)?;
                writer.write_str(" as ")?;
                write_constructor(pattern, writer)
            }
            ConstructorPattern::Constructor(constructor_type, variables) => {
                writer.write_display(constructor_type)?;
                writer.write_str("(")?;

                for (idx, pattern) in variables.iter().enumerate() {
                    if idx != 0 {
                        writer.write_str(",")?;
                    }
                    write_constructor(pattern, writer)?;
                }

                writer.write_str(")")
            }
            ConstructorPattern::Literal(expr) => match *expr.clone() {
                Expr::Variable(s) => writer.write_str(s),
                any_expr => writer.write_expr(&any_expr),
            },
        }
    }
}
