// Copyright (C) 2025 International Digital Economy Academy
//
// This program is free software; you can redistribute it and/or
// modify it under the terms of the GNU General Public License
// as published by the Free Software Foundation; version 2.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.
//
// You should have received a copy of the GNU General Public License
// along with this program; if not, see <https://www.gnu.org/licenses/>.

///|
test "@small_int_set.add/basic" {
  let empty = @small_int_set.empty()
  let s1 = empty.add(0)
  inspect(s1.contains(0), content="true")
  let s2 = s1.add(1)
  inspect(s2.contains(0), content="true")
  inspect(s2.contains(1), content="true")
}

///|
test "@small_int_set.is_empty" {
  // Test empty set
  let empty = @small_int_set.empty()
  inspect(empty.is_empty(), content="true")

  // Test non-empty set by adding elements
  let nonempty = empty.add(1)
  inspect(nonempty.is_empty(), content="false")

  // Test set after removing all elements
  let removed = nonempty.remove(1)
  inspect(removed.is_empty(), content="true")
}

///|
test "@small_int_set.iter/empty" {
  // Test iterating over an empty set
  let empty_set = @small_int_set.empty()
  let elements = empty_set.iter().to_array()
  inspect(elements, content="[]")
}

///|
test "@small_int_set.singleton/boundary" {
  // Test minimum reasonable input
  let set0 = @small_int_set.singleton(0)
  inspect(set0.contains(0), content="true")
  inspect(set0.contains(1), content="false")

  // Test boundary of first word
  let set31 = @small_int_set.singleton(31)
  inspect(set31.contains(31), content="true")
  inspect(set31.contains(30), content="false")

  // Test start of second word
  let set32 = @small_int_set.singleton(32)
  inspect(set32.contains(32), content="true")
  inspect(set32.contains(31), content="false")
}

///|
test "@small_int_set.iter/single_word" {
  // Test iterating over elements in a single word (0-31)
  let mut set = @small_int_set.empty()
  set = set.add(0)
  set = set.add(1)
  set = set.add(31)
  let elements = set.iter().to_array()
  inspect(elements, content="[0, 1, 31]")
}

///|
test "@small_int_set.iter/singleton" {
  let set = @small_int_set.singleton(64)
  let elements = set.iter().to_array()
  inspect(elements, content="[64]")
}

///|
test "@small_int_set.iter/multi_word" {
  // Test iterating over elements spanning multiple words (crossing 32-bit boundary)
  let mut set = @small_int_set.empty()
  set = set.add(31)
  set = set.add(32)
  set = set.add(63)
  let elements = set.iter().to_array()
  inspect(elements, content="[31, 32, 63]")
}

///|
test "@small_int_set.from_iter/basic" {
  let set = @small_int_set.from_iter([1, 2, 3].iter())
  inspect(set.contains(1), content="true")
  inspect(set.contains(2), content="true")
  inspect(set.contains(3), content="true")
  inspect(set.contains(4), content="false")
}

///|
test "@small_int_set.from_iter/empty" {
  let set = @small_int_set.from_iter(Iter::empty())
  inspect(set.is_empty(), content="true")
}

///|
test "@small_int_set.from_iter/multi_word" {
  // Test with numbers that span multiple words (32-bit boundaries)
  let set = @small_int_set.from_iter([31, 32, 33, 64].iter())
  inspect(set.contains(31), content="true")
  inspect(set.contains(32), content="true")
  inspect(set.contains(33), content="true")
  inspect(set.contains(64), content="true")
  inspect(set.contains(34), content="false")
}

///|
test "@small_int_set.add/word_boundary" {
  let empty = @small_int_set.empty()
  let s1 = empty.add(31)
  let s2 = s1.add(32)
  inspect(s2.contains(31), content="true")
  inspect(s2.contains(32), content="true")
  inspect(s2.contains(33), content="false")
}

///|
test "@small_int_set.add/idempotent" {
  let empty = @small_int_set.empty()
  let s1 = empty.add(5)
  let s2 = s1.add(5)
  inspect(s1.contains(5), content="true")
  inspect(s2.contains(5), content="true")
}

///|
test "@small_int_set.remove basic" {
  // Test basic remove functionality
  let set = @small_int_set.empty()
  let set1 = set.add(5)
  let set2 = set1.add(10)
  let result = set2.remove(5)
  inspect(result.contains(5), content="false")
  inspect(result.contains(10), content="true")
}

///|
test "@small_int_set.remove" {
  // Test removing the last element in a word
  let set = @small_int_set.singleton(1)
  inspect(set.remove(1), content="{}")

  // Test removing an element from a word that has multiple elements
  let set2 = @small_int_set.singleton(1).add(2)
  inspect(set2.remove(1), content="{2}")

  // Test removing a non-existent element
  let set3 = @small_int_set.singleton(1)
  inspect(set3.remove(33), content="{1}")
}

///|
test "@small_int_set.remove/non_existent" {
  // Test removing non-existent element should return same set
  let set = @small_int_set.empty()
  let set1 = set.add(1)
  let result = set1.remove(42)
  inspect(result.contains(1), content="true")
  // Test that removing non-existent element doesn't modify the set
  inspect(set1.contains(1), content="true")
}

///|
test "@small_int_set.remove/boundary" {
  // Test removing elements at word boundaries
  let set = @small_int_set.empty()
  let set1 = set.add(31)
  let set2 = set1.add(32)
  let set3 = set2.add(33)
  let result = set3.remove(32)
  inspect(result.contains(31), content="true")
  inspect(result.contains(32), content="false")
  inspect(result.contains(33), content="true")
}

///|
test "@small_int_set.union/empty_sets" {
  let empty = @small_int_set.empty()
  let result = empty.union(empty)

  // Empty union should contain no elements
  for i = 0; i < 32; i = i + 1 {
    inspect(result.contains(i), content="false")
  }
}

///|
test "@small_int_set.union/disjoint_sets" {
  let set1 = @small_int_set.empty() |> _.add(1) |> _.add(3)
  let set2 = @small_int_set.empty() |> _.add(2) |> _.add(4)
  let union = set1.union(set2)
  inspect(union.contains(1), content="true")
  inspect(union.contains(2), content="true")
  inspect(union.contains(3), content="true")
  inspect(union.contains(4), content="true")
  inspect(union.contains(5), content="false")
}

///|
test "@small_int_set.union/overlapping_sets" {
  let set1 = @small_int_set.empty() |> _.add(1) |> _.add(2)
  let set2 = @small_int_set.empty() |> _.add(2) |> _.add(3)
  let union = set1.union(set2)
  inspect(union.contains(1), content="true")
  inspect(union.contains(2), content="true")
  inspect(union.contains(3), content="true")
  inspect(union.contains(4), content="false")
}

///|
test "@small_int_set.union" {
  // Case 1: Tests when ib < jb
  // Create a set with elements that will have a lower word index
  let set1 = @small_int_set.singleton(1)
  // Create a set with elements that will have a higher word index
  let set2 = @small_int_set.singleton(64)
  let union1 = set1.union(set2)
  // Both elements should be in the union
  inspect(union1.contains(1), content="true")
  inspect(union1.contains(64), content="true")

  // Case 2: Tests when ib > jb
  // Now test with sets in reverse order
  let union2 = set2.union(set1)
  // Should get the same result regardless of order
  inspect(union2.contains(1), content="true")
  inspect(union2.contains(64), content="true")

  // Case 3: Tests when elements are in same word (ib = jb)
  let set3 = @small_int_set.singleton(2)
  let union3 = set1.union(set3)
  inspect(union3.contains(1), content="true")
  inspect(union3.contains(2), content="true")
}

///|
test "@small_int_set.intersection/empty_sets" {
  let empty = @small_int_set.empty()
  let result = empty.intersection(empty)
  // Empty set intersection with empty set should give empty set
  // Test by checking no elements from 0 to 31 exist
  for i = 0; i < 32; i = i + 1 {
    inspect(result.contains(i), content="false")
  }
}

///|
test "@small_int_set.intersection/disjoint_sets" {
  // Create two sets with no common elements
  let set1 = @small_int_set.empty().add(1) |> _.add(3) |> _.add(5)
  let set2 = @small_int_set.empty() |> _.add(2) |> _.add(4) |> _.add(6)
  let result = set1.intersection(set2)
  // Test that no elements from either set exist in the intersection
  let test_values = [1, 2, 3, 4, 5, 6]
  for i = 0; i < test_values.length(); i = i + 1 {
    inspect(result.contains(test_values[i]), content="false")
  }
}

///|
test "@small_int_set.intersection/overlapping_sets" {
  // Create two sets with some common elements
  let set1 = @small_int_set.empty().add(1) |> _.add(2) |> _.add(3)
  let set2 = @small_int_set.empty().add(2) |> _.add(3) |> _.add(4)
  let result = set1.intersection(set2)
  // Test common elements are present
  inspect(result.contains(2), content="true")
  inspect(result.contains(3), content="true")
  // Test non-common elements are absent
  inspect(result.contains(1), content="false")
  inspect(result.contains(4), content="false")
}

///|
test "@small_int_set.intersection" {
  // Test case 1: Test when ib < jb
  // Create two sets where the first set has a smaller word index
  let set1 = @small_int_set.singleton(1) // This will set bit in first word
  let set2 = @small_int_set.singleton(33) // This will set bit in second word
  assert_eq(set1.intersection(set2), @small_int_set.empty())

  // Test case 2: Test when ib > jb
  // Create two sets where the first set has a larger word index
  let set3 = @small_int_set.singleton(65) // This will set bit in third word
  let set4 = @small_int_set.singleton(33) // This will set bit in second word
  assert_eq(set3.intersection(set4), @small_int_set.empty())

  // Test case 3: Test when we need to remove empty words
  // Create two sets with overlapping bits to test word removal
  let set5 = @small_int_set.singleton(1).add(2)
  let set6 = @small_int_set.singleton(2).add(3)
  assert_eq(set5.intersection(set6), @small_int_set.singleton(2))
}

///|
test "@small_int_set.difference/empty_sets" {
  let empty = @small_int_set.empty()
  let result = empty.difference(empty)
  // An empty set minus an empty set should contain no elements
  for i = 0; i < 32; i = i + 1 {
    inspect(result.contains(i), content="false")
  }
}

///|
test "@small_int_set.difference/disjoint_sets" {
  // Create two disjoint sets {0,1,2} and {3,4,5}
  let mut set1 = @small_int_set.empty()
  let mut set2 = @small_int_set.empty()
  for i = 0; i < 3; i = i + 1 {
    set1 = set1.add(i)
  }
  for i = 3; i < 6; i = i + 1 {
    set2 = set2.add(i)
  }
  let result = set1.difference(set2)
  // The difference should contain all elements from set1
  for i = 0; i < 3; i = i + 1 {
    inspect(result.contains(i), content="true")
  }
  // And none from set2
  for i = 3; i < 6; i = i + 1 {
    inspect(result.contains(i), content="false")
  }
}

///|
test "@small_int_set.difference/overlapping_sets" {
  // Create two overlapping sets {0,1,2} and {1,2,3}
  let mut set1 = @small_int_set.empty()
  let mut set2 = @small_int_set.empty()
  for i = 0; i < 3; i = i + 1 {
    set1 = set1.add(i)
  }
  for i = 1; i < 4; i = i + 1 {
    set2 = set2.add(i)
  }
  let result = set1.difference(set2)
  // The difference should only contain 0
  inspect(result.contains(0), content="true")
  for i = 1; i < 4; i = i + 1 {
    inspect(result.contains(i), content="false")
  }
}

///|
test "@small_int_set.difference" {
  // Test case 1: when ib < jb
  // Create a set with element in lower word index and another with element in higher word index
  let set1 = @small_int_set.singleton(1) // word index 0
  let set2 = @small_int_set.singleton(64) // word index 2
  inspect(set1.difference(set2), content="{1}")

  // Test case 2: when ib > jb
  let set3 = @small_int_set.singleton(64) // word index 2
  let set4 = @small_int_set.singleton(1) // word index 0
  inspect(set3.difference(set4), content="{64}")

  // Test case 3: when result[k] == 0 and elements need to be removed
  // Create two sets with same word index but different bits
  let set5 = @small_int_set.singleton(32) // word index 1
  let set6 = @small_int_set.singleton(32) // word index 1
  inspect(set5.difference(set6), content="{}")
}

///|
test "@small_int_set.disjoint/empty_sets" {
  let empty1 = @small_int_set.empty()
  let empty2 = @small_int_set.empty()
  inspect(empty1.disjoint(empty2), content="true")
}

///|
test "@small_int_set.disjoint/non_overlapping" {
  // Create two sets with elements in different words (chunks of 32 bits)
  let mut set1 = @small_int_set.empty()
  set1 = set1.add(0)
  set1 = set1.add(1)
  let mut set2 = @small_int_set.empty()
  set2 = set2.add(33)
  set2 = set2.add(34)
  inspect(set1.disjoint(set2), content="true")
}

///|
test "@small_int_set.disjoint/overlapping" {
  let mut set1 = @small_int_set.empty()
  set1 = set1.add(5)
  set1 = set1.add(10)
  let mut set2 = @small_int_set.empty()
  set2 = set2.add(10)
  set2 = set2.add(15)
  inspect(set1.disjoint(set2), content="false")
}

///|
test "@small_int_set.disjoint" {
  // Test case 1: Sets with overlapping elements in the same word
  let set1 = @small_int_set.singleton(1).add(2)
  let set2 = @small_int_set.singleton(2).add(3)
  inspect(set1.disjoint(set2), content="false")

  // Test case 2: Sets with elements in different words
  let set3 = @small_int_set.singleton(1) // first word
  let set4 = @small_int_set.singleton(33) // second word
  inspect(set3.disjoint(set4), content="true")

  // Test case 3: Empty sets
  let empty1 = @small_int_set.empty()
  let empty2 = @small_int_set.empty()
  inspect(empty1.disjoint(empty2), content="true")
}

///|
test "@small_int_set.subset" {
  // Empty set is subset of any set
  let empty = @small_int_set.empty()
  let set1 = @small_int_set.from_iter([1, 2, 3].iter())
  inspect(empty.subset(set1), content="true")

  // Set is subset of itself
  inspect(set1.subset(set1), content="true")

  // Proper subset testing
  let set2 = @small_int_set.from_iter([1, 2].iter())
  inspect(set2.subset(set1), content="true")

  // Not a subset testing
  let set3 = @small_int_set.from_iter([1, 4].iter())
  inspect(set3.subset(set1), content="false")

  // Testing with elements across word boundaries (> 32)
  let large_set1 = @small_int_set.from_iter([1, 33, 65].iter())
  let large_set2 = @small_int_set.from_iter([1, 33, 65, 97].iter())
  inspect(large_set1.subset(large_set2), content="true")
  inspect(large_set2.subset(large_set1), content="false")
}

///|
test "@small_int_set.subset: bad case" {
  inspect(
    @small_int_set.from_iter([64].iter()).subset(
      @small_int_set.from_iter([21, 48, 58, 61, 81].iter()),
    ),
    content="false",
  )
}

///|
test "@small_int_set.is_singleton" {
  // Test with empty set
  let empty = @small_int_set.empty()
  inspect(empty.is_singleton(0), content="false")

  // Test with singleton set
  let single = @small_int_set.singleton(42)
  inspect(single.is_singleton(42), content="true")
  inspect(single.is_singleton(0), content="false")

  // Test with multiple elements
  let multi = @small_int_set.singleton(1).add(2)
  inspect(multi.is_singleton(1), content="false")
  inspect(multi.is_singleton(2), content="false")
}

///|
test "@small_int_set.is_singleton/boundary" {
  // Test with boundary values for word index and bit position
  let max_word = @small_int_set.singleton(31) // last bit in first word
  inspect(max_word.is_singleton(31), content="true")
  let min_next_word = @small_int_set.singleton(32) // first bit in second word
  inspect(min_next_word.is_singleton(32), content="true")
}
