#!/usr/bin/env node
/**
 * Stdio-to-HTTP MCP bridge for Claude Desktop.
 * Reads JSON-RPC from stdin, POSTs to the agent-b HTTP endpoint,
 * writes the response to stdout. No mcp-remote or OAuth needed.
 */
import { createInterface } from "readline";

const TARGET_URL = process.env.MCP_SERVER_URL || "http://localhost:8001/mcp";

const rl = createInterface({ input: process.stdin, crlfDelay: Infinity });

rl.on("line", async (line) => {
  const trimmed = line.trim();
  if (!trimmed) return;

  try {
    const req = JSON.parse(trimmed);
    const isNotification = req.id === undefined || req.id === null;

    const resp = await fetch(TARGET_URL, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: trimmed,
    });
    const text = await resp.text();

    // Notifications expect no response — don't forward to Claude
    if (isNotification) {
      process.stderr.write(`[mcp-stdio-bridge] Notification '${req.method}' acked (${resp.status})\n`);
      return;
    }

    if (text) {
      process.stdout.write(text + "\n");
    }
  } catch (err) {
    process.stderr.write(`[mcp-stdio-bridge] Error: ${err.message}\n`);
    try {
      const req = JSON.parse(trimmed);
      if (req.id !== undefined && req.id !== null) {
        const errorResp = JSON.stringify({
          jsonrpc: "2.0",
          id: req.id,
          error: { code: -32000, message: `Bridge error: ${err.message}` },
        });
        process.stdout.write(errorResp + "\n");
      }
    } catch { /* not valid JSON, nothing to respond to */ }
  }
});

process.stderr.write(`[mcp-stdio-bridge] Proxying to ${TARGET_URL}\n`);
