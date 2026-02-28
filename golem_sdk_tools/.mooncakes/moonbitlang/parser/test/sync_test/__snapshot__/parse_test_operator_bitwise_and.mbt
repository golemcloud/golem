// The truth of operators' precedence and associativity are extracted from parser.mly and unicode_lex.ml.
// Precedences of operators in ascending order:
// +-------------+-----------------+---------------+
// | Precedence  |    Operators    | Associativity |
// +-------------+-----------------+---------------+
// | logic or    | ||              | right         |
// | logic and   | &&              | right         |
// | bitwise or  | |               | left          |
// | bitwise xor | ^               | left          |
// | bitwise and | &               | left          |
// | INFIX1      | > < == != <= >= | left          |
// | INFIX2      | << >>           | left          |
// | PLUS MINUS  | + -             | left          |
// | INFIX3      | * / %           | left          |
// | unary ops   | +(_) -(_)       |               |
// +-------------+-----------------+---------------+
// Last update: June 27, 2024


// test operator &
fn test_op(){
    a & b && c
    a & b || c
    a & b > c
    a & b < c
    a & b == c
    a & b != c
    a & b <= c
    a & b >= c
    a & b + c
    a & b - c
    a & b * c
    a & b / c
    a & b % c
    a & b ^ c
    a & b | c
    a & b << c
    a & b >> c
}

// a @ (b @ c)
fn test_op(){
    a & (b && c)
    a & (b || c)
    a & (b > c)
    a & (b < c)
    a & (b == c)
    a & (b != c)
    a & (b <= c)
    a & (b >= c)
    a & (b + c)
    a & (b - c)
    a & (b * c)
    a & (b / c)
    a & (b % c)
    a & (b ^ c)
    a & (b | c)
    a & (b << c)
    a & (b >> c)
}

// (a @ b) @ c
fn test_op(){
    (a & b) && c
    (a & b) || c
    (a & b) > c
    (a & b) < c
    (a & b) == c
    (a & b) != c
    (a & b) <= c
    (a & b) >= c
    (a & b) + c
    (a & b) - c
    (a & b) * c
    (a & b) / c
    (a & b) % c
    (a & b) ^ c
    (a & b) | c
    (a & b) << c
    (a & b) >> c
}

fn test_op(){
    a & (b & (c & (d & (e & f))))
    a & (b & c & (d & e & f))
    a & b & c & d & f
}