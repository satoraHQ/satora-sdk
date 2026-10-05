// biome-ignore lint/correctness/useImportExtensions: a JSON module keeps its extension
import vectors from "../../../fixtures/lightning-invoices.json";

/**
 * Real invoices and swap responses, kept outside this package so other SDKs
 * can test against the same bytes.
 */
export default vectors;
