
trait I: S { f(Self) }
trait I: S1 + S2 { f(Self) }
trait I: S + { f(Self) }
trait I: { f(Self) }
