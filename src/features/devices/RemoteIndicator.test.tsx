import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { render, screen } from "@testing-library/react";
import { t } from "@/lib/i18n";
import { mockRemote, resetVaultMock } from "@/lib/mock";
import { RemoteIndicator } from "./RemoteIndicator";

beforeEach(() => resetVaultMock());

const setup = () =>
  render(
    <QueryClientProvider client={new QueryClient()}>
      <RemoteIndicator />
    </QueryClientProvider>,
  );

test.each([
  [false, "rs.liveView"],
  [true, "rs.liveControl"],
] as const)("control %s: says viewing or controlling, never 'PC'", async (control, key) => {
  mockRemote.session = { peer: "Laptop", since: "2026-11-02T18:30:00Z", control };
  setup();
  expect(await screen.findByText(t(key, { peer: "Laptop" }))).toHaveAttribute("role", "status");
  expect(screen.getByRole("status")).not.toHaveTextContent("PC");
});
