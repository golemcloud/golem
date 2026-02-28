const Int_arith_1 : Int = Int_arith_2 * Int_arith_3
const Int_arith_2 : Int = 11 + Int_arith_3
const Int_arith_3 : Int = Int_arith_4 % -(2 * -2)
const Int_arith_4 : Int = 9 - (10 / 5)

fn init {
  println(Int_arith_1)
}

const Mask_hi16 : UInt = (0x0000ffff : UInt) << 16
const Mask_lo16 : UInt = (0xffffffff : UInt) >> 16
const UInt_bitwise : UInt = (Mask_lo16 & 0x33333333) | (Mask_hi16 & 0x55555555)

fn init {
  println(UInt_bitwise.to_string(radix=16))
}

const Double_arith : Double = (0.5 * 7 + 1.9 - 0.78) / 1.1

fn init {
  println(Double_arith)
}

const Int_comparison : Bool = Int_arith_1 < Int_arith_4
const UInt_comparison : Bool = Mask_lo16 >= Mask_hi16
const Double_comparison : Bool = Double_arith == 0

fn init {
  println(Int_comparison)
  println(UInt_comparison)
  println(Double_comparison)
}
