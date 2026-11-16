import { barOf } from "./usePlay";

test("barOf drops a leftover server when the destination is empty", () => {
  expect(barOf("", " job ", "specific")).toEqual({
    destination: "",
    server: "",
    mode: "specific",
    home: true,
  });
  expect(barOf(" 1818 ", " job ", "specific")).toEqual({
    destination: "1818",
    server: "job",
    mode: "specific",
    home: true,
  });
});
