// Bundles the client into one ESM file: the Codama output imports without file extensions, which
// bundlers resolve and Node's ESM loader does not.
import { defineConfig } from "tsup";

export default defineConfig({
  entry: ["src/index.ts"],
  format: ["esm"],
  dts: true,
  sourcemap: true,
  clean: true,
  target: "es2022",
  external: ["@solana/kit", "@solana/program-client-core"],
});
