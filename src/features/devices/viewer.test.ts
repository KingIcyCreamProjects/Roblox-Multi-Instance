/// <reference types="node" />
// The remote viewer is shared with HomeLab byte for byte (public/remote-viewer/). This fails when
// the copy here changes without tools/sync-remote-viewer.mjs recording it, so the two apps
// cannot drift silently. Updating: run that script with the HomeLab checkout, commit both files.
import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import recorded from "./viewer.sha256.json";

const dir = join(__dirname, "../../../public/remote-viewer");

test.each(Object.entries(recorded))("%s matches the recorded HomeLab hash", (file, sha) => {
  const got = createHash("sha256")
    .update(readFileSync(join(dir, file)))
    .digest("hex");
  expect(got).toBe(sha);
});

test("the viewer API this app calls is there", () => {
  const js = readFileSync(join(dir, "remote-viewer.js"), "utf8");
  expect(js).toContain("window.RemoteViewer = { open }");
  expect(js).toMatch(/function open\(\{ mount, url, auth, title[^}]*, onClose \}\)/);
});
