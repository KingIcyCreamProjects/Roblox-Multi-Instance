import * as Tooltip from "@radix-ui/react-tooltip";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import type { ReactElement } from "react";
import { t } from "@/lib/i18n";
import { PasswordField } from "./PasswordField";

// IconButton's tooltip needs a provider
const wrap = (el: ReactElement) => <Tooltip.Provider>{el}</Tooltip.Provider>;

test("show toggle flips the input type and its pressed state", async () => {
  const user = userEvent.setup();
  render(wrap(<PasswordField label="Pw" value="secret" onChange={() => {}} />));
  const input = screen.getByLabelText("Pw");
  expect(input).toHaveAttribute("type", "password");
  await user.click(screen.getByRole("button", { name: t("vault.show") }));
  expect(input).toHaveAttribute("type", "text");
  expect(screen.getByRole("button", { name: t("vault.hide") })).toHaveAttribute(
    "aria-pressed",
    "true",
  );
});

test("an error marks the input invalid and announces; a hint stays quiet", () => {
  const { rerender } = render(
    wrap(<PasswordField label="Pw" value="" onChange={() => {}} error="Nope" hint="Tip" />),
  );
  expect(screen.getByLabelText("Pw")).toHaveAttribute("aria-invalid", "true");
  // the error wins over the hint
  expect(screen.getByRole("alert")).toHaveTextContent("Nope");
  expect(screen.queryByText("Tip")).toBeNull();
  rerender(wrap(<PasswordField label="Pw" value="" onChange={() => {}} hint="Tip" />));
  expect(screen.getByLabelText("Pw")).toHaveAttribute("aria-invalid", "false");
  expect(screen.queryByRole("alert")).toBeNull();
  expect(screen.getByText("Tip")).toBeInTheDocument();
});
