import { useDraft, draftPatch } from "./draft";

beforeEach(() => useDraft.getState().clear());

test("an edit equal to the saved value drops the entry; NaN equals NaN", () => {
  const { edit } = useDraft.getState();
  edit("ui.scale", 110, 100);
  expect(useDraft.getState().edits).toEqual({ "ui.scale": 110 });
  edit("ui.scale", 100, 100);
  expect(useDraft.getState().edits).toEqual({});
  edit("x.y", NaN, NaN);
  expect(useDraft.getState().edits).toEqual({});
});

test("dropSent removes only edits that went out unchanged", () => {
  useDraft.getState().setAll({ "a.b": 1, "a.c": 2 });
  // "a.b" was typed again while the save was in flight: keep it
  useDraft.getState().dropSent({ "a.b": 9, "a.c": 2 });
  expect(useDraft.getState().edits).toEqual({ "a.b": 1 });
});

test("draftPatch nests sibling paths into one patch", () => {
  expect(draftPatch({ "a.b": 1, "a.c": 2, top: true })).toEqual({
    a: { b: 1, c: 2 },
    top: true,
  });
});
