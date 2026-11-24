// @ts-check
import eslint from "@eslint/js";
import { defineConfig } from "eslint/config";
import tseslint from "typescript-eslint";

export default defineConfig(
    { ignores: ["out/", "**/*.d.ts"] },
    eslint.configs.recommended,
    tseslint.configs.recommended,
    {
        rules: {
            curly: "warn",
            eqeqeq: "warn",
            "no-throw-literal": "warn",
        },
    },
);
