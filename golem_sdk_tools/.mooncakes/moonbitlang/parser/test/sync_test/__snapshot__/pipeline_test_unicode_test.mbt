
enum T_类型 {
    T(Int)
}

fn 斐波那契数列(第几个数: Int) -> Int {
    let mut 前一个 = 0
    let mut 后一个 = 1
    let mut 计数器 = 1
    while 计数器 < 第几个数 {
        let 中间 = 前一个 + 后一个
        前一个 = 后一个
        后一个 = 中间
        计数器 = 计数器 + 1
    }
    前一个
}

fn 相加(左: Int, 右: Int) -> Int {
    左 + 右
}

fn T_类型::打印(self: T_类型) -> Unit {
    let T(x) = self
    println(x)
}

fn T_类型::方法() -> T_类型 {
    T_类型::T(1)
}

fn init {
    let 答案 = 斐波那契数列(15)
    T_类型::T(相加(答案, 42)).打印()
    let falsex = false
    if falsex {
        let truey = true
        println("aa")
    }
    let 😅 = 114514
    println(😅)
}

enum T_测试枚举 {
  E无
  E有(String)
}

fn T_测试枚举::to_string(self: T_测试枚举) -> String {
  match self {
    T_测试枚举::E无 => "None"
    T_测试枚举::E有(t) => "Some(\{t})"
  }
}
