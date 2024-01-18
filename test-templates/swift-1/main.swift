import Foundation

let date = Date()
let calendar = Calendar.current

let year = calendar.component(.year, from: date)

print("Hello world!")
print(year)
