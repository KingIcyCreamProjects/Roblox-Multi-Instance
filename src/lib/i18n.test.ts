import { t } from "./i18n";

test("t fills each placeholder once and never reads a value as a replacement pattern", () => {
  expect(t("launch.ok", { name: "a$&b", game: "Money $$$ Tycoon" })).toBe(
    "Launched a$&b into Money $$$ Tycoon",
  );
  // a value that looks like a placeholder is not substituted again
  expect(t("launch.failed", { name: "{why}", why: "boom" })).toBe("{why}: boom");
});
