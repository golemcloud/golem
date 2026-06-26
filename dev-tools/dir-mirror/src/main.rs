// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use std::path::Path;
use std::process::ExitCode;

use dir_mirror::mirror;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() || !args.len().is_multiple_of(2) {
        eprintln!(
            "Usage: dir-mirror <src1> <dst1> [<src2> <dst2> ...]\n\
             \n\
             Makes each <dst> a byte-identical copy of <src>: unchanged files are left\n\
             untouched (preserving their mtime), and files/dirs in <dst> absent from\n\
             <src> are removed."
        );
        return ExitCode::FAILURE;
    }

    for pair in args.chunks(2) {
        let (src, dst) = (Path::new(&pair[0]), Path::new(&pair[1]));
        if let Err(e) = mirror(src, dst) {
            eprintln!("dir-mirror: {} -> {}: {e}", pair[0], pair[1]);
            return ExitCode::FAILURE;
        }
    }
    ExitCode::SUCCESS
}
