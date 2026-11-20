// The reader lives in js/ as the mokuji package. The pages here import
// its built output through this file, so that they and everything else read
// Mokuji files with the same code. serve.py serves js/ for the browser.
export * from "../../js/dist/index.js";
