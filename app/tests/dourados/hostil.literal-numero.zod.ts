import { z } from "zod";

export const Registro = z.object({
  "a\"b": z.literal("a\"b"),
  "c\\d": z.literal(1),
  "\u003c/script\u003e\u003cscript\u003ealert(1)\u003c/script\u003e": z.literal("\u003c/script\u003e\u003cscript\u003ealert(1)\u003c/script\u003e"),
  "\u0007sino": z.literal(3),
  "\u202etxt.exe": z.literal("\u202etxt.exe"),
  "linha\u2028separada": z.literal(5),
  ["__proto__"]: z.literal("__proto__"),
  constructor: z.literal(7),
  "${x}": z.literal("${x}"),
  "`crase`": z.literal(9),
  "\u200bzero": z.literal("\u200bzero"),
  "\u0085c1": z.literal(11),
  ok: z.literal("ok"),
});

export type Registro = z.infer<typeof Registro>;
