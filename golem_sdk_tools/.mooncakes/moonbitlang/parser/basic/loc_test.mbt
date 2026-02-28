///|
test "Location::merge - basic merge with different lines" {
  // Location 1: line 5-6, Location 2: line 8-9
  // Expected: merged location from line 5 to line 9
  let loc1 = @basic.Location::{
    start: @basic.Position::{ fname: "test.mbt", lnum: 5, bol: 50, cnum: 55 },
    end: @basic.Position::{ fname: "test.mbt", lnum: 6, bol: 60, cnum: 65 },
  }
  let loc2 = @basic.Location::{
    start: @basic.Position::{ fname: "test.mbt", lnum: 8, bol: 80, cnum: 85 },
    end: @basic.Position::{ fname: "test.mbt", lnum: 9, bol: 90, cnum: 95 },
  }
  inspect(loc1, content="test.mbt-5:6-6:6")
  inspect(loc2, content="test.mbt-8:6-9:6")
  inspect(loc1.merge(loc2), content="test.mbt-5:6-9:6")
  inspect(loc2.merge(loc1), content="test.mbt-5:6-9:6")
}

///|
test "Location::merge - overlapping locations" {
  // Location 1: line 5-8, Location 2: line 6-10
  // Expected: merged location from line 5 to line 10
  let loc1 = @basic.Location::{
    start: @basic.Position::{ fname: "test.mbt", lnum: 5, bol: 50, cnum: 55 },
    end: @basic.Position::{ fname: "test.mbt", lnum: 8, bol: 80, cnum: 85 },
  }
  let loc2 = @basic.Location::{
    start: @basic.Position::{ fname: "test.mbt", lnum: 6, bol: 60, cnum: 65 },
    end: @basic.Position::{ fname: "test.mbt", lnum: 10, bol: 100, cnum: 105 },
  }
  inspect(loc1, content="test.mbt-5:6-8:6")
  inspect(loc2, content="test.mbt-6:6-10:6")
  inspect(loc1.merge(loc2), content="test.mbt-5:6-10:6")
  inspect(loc2.merge(loc1), content="test.mbt-5:6-10:6")
}

///|
test "Location::merge - same line different bol for start positions" {
  // Both locations start on same line but different bol
  // Expected: take the one with smaller bol for start
  let loc1 = @basic.Location::{
    start: @basic.Position::{ fname: "test.mbt", lnum: 5, bol: 60, cnum: 65 },
    end: @basic.Position::{ fname: "test.mbt", lnum: 6, bol: 70, cnum: 75 },
  }
  let loc2 = @basic.Location::{
    start: @basic.Position::{ fname: "test.mbt", lnum: 5, bol: 50, cnum: 55 },
    end: @basic.Position::{ fname: "test.mbt", lnum: 7, bol: 80, cnum: 85 },
  }
  inspect(loc1, content="test.mbt-5:6-6:6")
  inspect(loc2, content="test.mbt-5:6-7:6")
  inspect(loc1.merge(loc2), content="test.mbt-5:6-7:6")
  inspect(loc2.merge(loc1), content="test.mbt-5:6-7:6")
}

///|
test "Location::merge - same line different bol for end positions" {
  // Both locations end on same line but different bol
  // Expected: take the one with larger bol for end
  let loc1 = @basic.Location::{
    start: @basic.Position::{ fname: "test.mbt", lnum: 3, bol: 30, cnum: 35 },
    end: @basic.Position::{ fname: "test.mbt", lnum: 5, bol: 60, cnum: 65 },
  }
  let loc2 = @basic.Location::{
    start: @basic.Position::{ fname: "test.mbt", lnum: 4, bol: 40, cnum: 45 },
    end: @basic.Position::{ fname: "test.mbt", lnum: 5, bol: 70, cnum: 75 },
  }
  inspect(loc1, content="test.mbt-3:6-5:6")
  inspect(loc2, content="test.mbt-4:6-5:6")
  inspect(loc1.merge(loc2), content="test.mbt-3:6-5:6")
  inspect(loc2.merge(loc1), content="test.mbt-3:6-5:6")
}

///|
test "Location::merge - identical locations" {
  // Merging identical locations should return the same location
  let loc = @basic.Location::{
    start: @basic.Position::{ fname: "test.mbt", lnum: 5, bol: 50, cnum: 55 },
    end: @basic.Position::{ fname: "test.mbt", lnum: 6, bol: 60, cnum: 65 },
  }
  inspect(loc, content="test.mbt-5:6-6:6")
  inspect(loc.merge(loc), content="test.mbt-5:6-6:6")
}

///|
test "Location::merge - single character locations" {
  // Test merging single character locations
  let loc1 = @basic.Location::{
    start: @basic.Position::{ fname: "test.mbt", lnum: 1, bol: 0, cnum: 5 },
    end: @basic.Position::{ fname: "test.mbt", lnum: 1, bol: 0, cnum: 5 },
  }
  let loc2 = @basic.Location::{
    start: @basic.Position::{ fname: "test.mbt", lnum: 1, bol: 0, cnum: 10 },
    end: @basic.Position::{ fname: "test.mbt", lnum: 1, bol: 0, cnum: 10 },
  }
  let merged = loc1.merge(loc2)

  // Should span from first character to second character
  inspect(loc1, content="test.mbt-1:6-1:6")
  inspect(loc2, content="test.mbt-1:11-1:11")
  inspect(merged, content="test.mbt-1:6-1:11")
}
