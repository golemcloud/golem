'use strict';

// Golem's QuickJS runtime has no V8 inspector protocol. TypeScript treats a module without Session
// as profiling unavailable and continues normally.
module.exports = Object.freeze({});
