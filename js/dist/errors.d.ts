/** A file, a manifest or a query refused: what the Rust reader's `Error` and
 * the Python reader's `MokujiError` say. */
export declare class MokujiError extends Error {
    name: string;
}
/** A search that names an attribute column the file does not carry. */
export declare class NoColumnError extends MokujiError {
    readonly column: string;
    name: string;
    constructor(column: string);
}
/** A builder refusing what it was given. */
export declare class BuildError extends Error {
    name: string;
}
