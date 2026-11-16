import { lastLaunchSet, placeIdOf, saveLaunchSet, targetFromBar, usePlayBar } from "./store";

const uuid = "0b9a7c2e-1f3d-4c5b-8a6e-2d4f6a8c0e1b";

test("targetFromBar keeps Roblox links and UUID JobIds only", () => {
  const share = "https://www.roblox.com/share?code=abc&type=Server";
  expect(targetFromBar(share, "", "any")).toEqual({
    placeId: null,
    jobId: null,
    privateLink: share,
  });
  const bare = "www.roblox.com/share?code=abc&type=Server";
  expect(targetFromBar(bare, "", "any")?.privateLink).toBe(bare);
  const ps = "https://www.roblox.com/games/1818/x?privateServerLinkCode=99";
  expect(targetFromBar(ps, "", "any")).toMatchObject({ placeId: 1818, privateLink: ps });
  const ro =
    "https://ro.blox.com/Ebh5?af_web_dp=https%3A%2F%2Fwww.roblox.com%2Fshare%3Fcode%3Dx%26type%3DServer";
  expect(targetFromBar(ro, "", "any")?.privateLink).toBe(ro);

  expect(targetFromBar("https://evil.example/share?code=abc", "", "any")).toBeNull();
  expect(targetFromBar("https://roblox.com.evil.example/share?x", "", "any")).toBeNull();
  expect(targetFromBar("@builderman", "", "any")).toBeNull();
  expect(targetFromBar("jailbreak", "", "any")).toBeNull();

  expect(targetFromBar("1818", uuid, "specific")).toEqual({
    placeId: 1818,
    jobId: uuid,
    privateLink: null,
  });
  expect(targetFromBar("1818", "not-a-job", "specific")).toBeNull();
  expect(targetFromBar("1818", uuid, "any")?.jobId).toBeNull();
});

test("targetFromBar keeps Random / Smallest on a game, never on a private link", () => {
  expect(targetFromBar("606849621", "", "smallest")).toEqual({
    placeId: 606849621,
    jobId: null,
    privateLink: null,
    mode: "smallest",
  });
  expect(
    targetFromBar("https://www.roblox.com/games/9?privateServerLinkCode=L", "", "random"),
  ).not.toHaveProperty("mode");
});

test("placeIdOf takes a bare id or a roblox.com/games link, nothing else", () => {
  expect(placeIdOf(" 1818 ")).toBe(1818);
  expect(placeIdOf("https://www.roblox.com/games/1818/Obby?x=1")).toBe(1818);
  expect(placeIdOf("www.roblox.com/games/42")).toBe(42);
  expect(placeIdOf("12ab")).toBeNull();
  expect(placeIdOf("roblox.com/users/42")).toBeNull();
  expect(placeIdOf("")).toBeNull();
});

test("a new destination drops the server pick and mode; server and mode keep the destination", () => {
  usePlayBar.setState({ destination: "1", server: "s", mode: "specific" });
  usePlayBar.getState().setServer("abc");
  expect(usePlayBar.getState()).toMatchObject({
    destination: "1",
    server: "abc",
    mode: "specific",
  });
  usePlayBar.getState().setMode("random");
  expect(usePlayBar.getState()).toMatchObject({ destination: "1", server: "abc", mode: "random" });
  usePlayBar.getState().setDestination("2");
  expect(usePlayBar.getState()).toMatchObject({ destination: "2", server: "", mode: "any" });
});

test("last launch set round-trips; unreadable storage reads as empty, bad entries dropped", () => {
  saveLaunchSet(["a", "b"]);
  expect(lastLaunchSet()).toEqual(["a", "b"]);
  localStorage.setItem("rmi.lastLaunchSet", '{"x":1}');
  expect(lastLaunchSet()).toEqual([]);
  localStorage.setItem("rmi.lastLaunchSet", '["a",2,null,"b"]');
  expect(lastLaunchSet()).toEqual(["a", "b"]);
  localStorage.setItem("rmi.lastLaunchSet", "{not json");
  expect(lastLaunchSet()).toEqual([]);
});

test("saving the last launch set never throws, even when storage refuses", () => {
  const spy = vi.spyOn(Storage.prototype, "setItem").mockImplementation(() => {
    throw new DOMException("full", "QuotaExceededError");
  });
  try {
    expect(() => saveLaunchSet(["a"])).not.toThrow();
  } finally {
    spy.mockRestore();
  }
});

test("links follow the Rust rules: games/start, locale, ro.blox.com, job links", () => {
  expect(placeIdOf("https://www.roblox.com/games/start?placeId=77&launchData=x")).toBe(77);
  expect(placeIdOf("https://www.roblox.com/pt-br/games/920587237/x")).toBe(920587237);
  const af = "https://ro.blox.com/Ebh5?af_web_dp=https%3A%2F%2Fwww.roblox.com%2Fgames%2F5";
  expect(placeIdOf(af)).toBe(5);
  // public game redirect: a place target, not a private link
  expect(targetFromBar(af, "", "any")).toEqual({ placeId: 5, jobId: null, privateLink: null });
  expect(targetFromBar("https://ro.blox.com/Ebh5", "", "any")).toBeNull();
  // game-instance link keeps its job whatever the mode
  const job = `https://www.roblox.com/games/9/x?gameInstanceId=${uuid}`;
  expect(targetFromBar(job, "", "any")).toEqual({ placeId: 9, jobId: uuid, privateLink: null });
  // the same link in the server field is the JobId
  expect(targetFromBar("9", job, "specific")?.jobId).toBe(uuid);
});

test("placeIdOf only trusts roblox.com links", () => {
  expect(placeIdOf("https://www.roblox.com/games/42/x")).toBe(42);
  expect(placeIdOf("roblox.com/games/42")).toBe(42);
  expect(placeIdOf("https://evil.example/roblox.com/games/42")).toBeNull();
});
