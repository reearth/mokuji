// What the library throws, so that a caller can tell a refusal from a bug.
//
// A file that breaks a rule of SPEC.md, a query the file cannot answer and a
// server that does not honour a range are refused with `MokujiError`; a
// builder given what it cannot write refuses with `BuildError`. Anything else
// that escapes — a TypeError, a RangeError — is the library tripping, and a
// bug.
/** A file, a manifest or a query refused: what the Rust reader's `Error` and
 * the Python reader's `MokujiError` say. */
export class MokujiError extends Error {
    name = "MokujiError";
}
/** A search that names an attribute column the file does not carry. */
export class NoColumnError extends MokujiError {
    column;
    name = "NoColumnError";
    constructor(column) {
        super(`this index carries no attribute column called '${column}'`);
        this.column = column;
    }
}
/** A builder refusing what it was given. */
export class BuildError extends Error {
    name = "BuildError";
}
