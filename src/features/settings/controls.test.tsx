import { fireEvent, render, screen } from "@testing-library/react";
import { t } from "@/lib/i18n";
import { Setting } from "./controls";
import type { Item } from "./catalog";

const delay: Item = {
  path: "launch.delayMs",
  kind: "num",
  min: 0,
  max: 60_000,
  scale: 1000,
  step: 0.5,
};
const scanSec: Item = { path: "watcher.scanSec", kind: "num", min: 2, max: 600 };
const url: Item = { path: "geo.urlTemplate", kind: "text" };
const customPath: Item = {
  path: "client.customSettingsPath",
  kind: "text",
  nullable: true,
  onBlur: true,
};

const label = (path: string) => t(`set.${path}` as never);

test("a seconds field shows seconds, rounds to its step on blur, and stores ms", () => {
  const onChange = vi.fn();
  render(<Setting item={delay} value={2500} reason={null} onChange={onChange} />);
  const input = screen.getByRole("spinbutton", { name: label(delay.path) });
  expect(input).toHaveValue(2.5);
  fireEvent.change(input, { target: { value: "3.2" } });
  expect(onChange).toHaveBeenLastCalledWith(3200);
  fireEvent.blur(input);
  // 3.2 rounds to the 0.5 step: 3.0 s
  expect(onChange).toHaveBeenLastCalledWith(3000);
  expect(input).toHaveValue(3);
});

test("an out-of-range number waits for blur, then clamps to the range", () => {
  const onChange = vi.fn();
  render(<Setting item={scanSec} value={6} reason={null} onChange={onChange} />);
  const input = screen.getByRole("spinbutton", { name: label(scanSec.path) });
  fireEvent.change(input, { target: { value: "9999" } });
  expect(onChange).not.toHaveBeenCalled();
  fireEvent.blur(input);
  expect(onChange).toHaveBeenLastCalledWith(600);
  expect(input).toHaveValue(600);
});

test("an emptied number reverts on blur and never reaches the draft", () => {
  const onChange = vi.fn();
  render(<Setting item={scanSec} value={6} reason={null} onChange={onChange} />);
  const input = screen.getByRole("spinbutton", { name: label(scanSec.path) });
  fireEvent.change(input, { target: { value: "" } });
  fireEvent.blur(input);
  expect(onChange).not.toHaveBeenCalled();
  expect(input).toHaveValue(6);
});

test("a plain text field keeps an emptied value as empty text", () => {
  const onChange = vi.fn();
  render(<Setting item={url} value="http://x/{ip}" reason={null} onChange={onChange} />);
  fireEvent.change(screen.getByRole("textbox", { name: label(url.path) }), {
    target: { value: "" },
  });
  expect(onChange).toHaveBeenLastCalledWith("");
});

test("a nullable on-blur text field saves null when emptied, and only on blur", () => {
  const onChange = vi.fn();
  render(<Setting item={customPath} value="C:/a.json" reason={null} onChange={onChange} />);
  const input = screen.getByRole("textbox", { name: label(customPath.path) });
  fireEvent.change(input, { target: { value: "" } });
  expect(onChange).not.toHaveBeenCalled();
  fireEvent.blur(input);
  expect(onChange).toHaveBeenCalledWith(null);
});

test("a warn item shows its note under the control", () => {
  render(
    <Setting
      item={{ path: "net.allowLan", kind: "bool", warn: "settings.lanWarn" }}
      value={false}
      reason={null}
      onChange={() => {}}
    />,
  );
  expect(screen.getByRole("note")).toHaveTextContent(t("settings.lanWarn"));
});

test("an enum setting shows its options and reports the chosen one", () => {
  const theme: Item = { path: "theme", kind: "enum", options: ["system", "dark", "light"] };
  const onChange = vi.fn();
  render(<Setting item={theme} value="system" reason={null} onChange={onChange} />);
  expect(screen.getByRole("option", { name: t("opt.dark") })).toBeInTheDocument();
  fireEvent.change(screen.getByRole("combobox", { name: label(theme.path) }), {
    target: { value: "dark" },
  });
  expect(onChange).toHaveBeenCalledWith("dark");
});
