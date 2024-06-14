// Copyright 2024 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

import { Result } from "./result";

/**
 * Workaround helper until async support arrives: runEventLoopUntilInterest keeps
 * running the event loop until all pending async tasks and jobs (e.g. Fetch promises)
 * are finished.
 */
export declare const runEventLoopUntilInterest: () => void;

/**
 * Waits for a promise (by using runEventLoopUntilInterest) and returns
 * its result synchronously. In case of rejection it rethrows the error.
 * @param promise
 */
export function asyncToSync<T>(promise: Promise<T>): T {
    let success = false;
    let done = false;
    let result: T;
    let error: any;

    promise
        .then((r) => {
            result = r;
            success = true;
            done = true;
        })
        .catch((e) => {
            error = e;
            done = true;
        });

    runEventLoopUntilInterest();

    if (!done) {
        throw new Error("asyncToSync: illegal state: not done");
    }

    if (!success) {
        throw error;
    }

    return result;
}

/**
 * Result returning variant of asyncToSync;
 * @param promise
 */
export function asyncToSyncAsResult<T>(promise: Promise<T>): Result<T, any> {
    return Result.tryCatch(() => asyncToSync(promise));
}
