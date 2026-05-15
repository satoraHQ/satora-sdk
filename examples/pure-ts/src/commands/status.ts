/**
 * Show detailed API and dependency status.
 */

import type { Client } from "@lendasat/lendaswap-sdk-pure";

export async function showStatus(client: Client): Promise<void> {
  const status = await client.getStatus();

  console.log("API Status");
  console.log("=".repeat(60));
  console.log(`Overall: ${status.healthy ? "healthy" : "unhealthy"}`);
  console.log("");
  console.log("Services:");

  for (const [name, service] of Object.entries(status.services)) {
    const state = service.healthy ? "healthy" : "unhealthy";
    const height = service.block_height ?? "n/a";
    const time = service.block_time ?? "n/a";
    const error = service.error ? ` (${service.error})` : "";

    console.log(
      `  ${name.padEnd(10)} ${state.padEnd(9)} height=${height} time=${time}${error}`,
    );
  }
}
