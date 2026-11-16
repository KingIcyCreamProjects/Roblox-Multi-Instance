import appSrc from "../../src-tauri/src/friend/web/app.js?raw";

// app.js is a plain page script: pull out the pure login error mapper and run it alone.
const noReach = appSrc.match(/^const NO_REACH.*$/m)?.[0] ?? "";
const fnSrc = appSrc.match(/^function loginError[\s\S]*?^\}/m)?.[0] ?? "";
const loginError = new Function(`${noReach}\n${fnSrc}; return loginError;`)() as (
  s: number,
) => string;

test("login errors: wrong creds, rate limit, offline", () => {
  expect(loginError(401)).toBe(
    "Wrong username or password. Check them, or ask the owner for a new password.",
  );
  expect(loginError(429)).toBe("Too many tries. Wait 1 minute, then try again.");
  expect(loginError(0)).toBe("Can't reach the owner's computer.");
  expect(loginError(502)).toBe("Can't reach the owner's computer."); // the tunnel: RMI is not running
  expect(loginError(500)).toBe("Something went wrong");
});

test("no sign-up and no token-in-link left in the page", () => {
  expect(appSrc).not.toMatch(/location\.hash|sign ?up|register/i);
});
