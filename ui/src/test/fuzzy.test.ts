import { expect, test } from "vitest";
import { rank } from "../fuzzy";

const items = [
  { id: "dash", title: "Dashboard" },
  { id: "wt", title: "acme/feat/login-form", keywords: ["feat/login-form"] },
  { id: "task", title: "Per-tenant rate limiting", keywords: ["#412"] },
  { id: "theme", title: "Toggle theme" },
];

test("an empty query keeps every item in order", () => {
  expect(rank("", items).map((i) => i.id)).toEqual(["dash", "wt", "task", "theme"]);
});

test("matches characters in order, not necessarily adjacent, ignoring case", () => {
  expect(rank("DBD", items).map((i) => i.id)).toEqual(["dash"]);
  expect(rank("flf", items).map((i) => i.id)).toEqual(["wt"]);
});

test("matches task keys and branches given as keywords", () => {
  expect(rank("412", items).map((i) => i.id)).toEqual(["task"]);
  expect(rank("login", items).map((i) => i.id)).toEqual(["wt"]);
});

test("contiguous and word-start matches outrank scattered ones", () => {
  const list = [
    { id: "scattered", title: "tab hint sheet" },
    { id: "contiguous", title: "theme" },
  ];
  expect(rank("the", list).map((i) => i.id)).toEqual(["contiguous", "scattered"]);
});

test("nothing matches when a character is missing", () => {
  expect(rank("zz", items)).toEqual([]);
});
