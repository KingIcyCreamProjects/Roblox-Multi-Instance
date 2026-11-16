import js from "@eslint/js";
import globals from "globals";
import tseslint from "typescript-eslint";
import reactHooks from "eslint-plugin-react-hooks";

export default tseslint.config(
  // public/remote-viewer: HomeLab's file, copied byte for byte (tools/sync-remote-viewer.mjs)
  {
    ignores: ["dist", "e2e/.drivers", "src-tauri", "node_modules", "docs", "public/remote-viewer"],
  },
  js.configs.recommended,
  ...tseslint.configs.recommended,
  {
    files: ["src/**/*.{ts,tsx}"],
    languageOptions: { globals: globals.browser },
    plugins: { "react-hooks": reactHooks },
    rules: {
      "react-hooks/rules-of-hooks": "error",
      "react-hooks/exhaustive-deps": "warn",
    },
  },
  { files: ["e2e/**/*.js"], languageOptions: { globals: { ...globals.node, ...globals.mocha } } },
  { files: ["public/**/*.js"], languageOptions: { globals: globals.browser } },
  { files: ["tools/**/*.mjs"], languageOptions: { globals: globals.node } },
);
