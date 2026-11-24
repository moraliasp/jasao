import { z } from "zod";

export const Registro = z.object({
  "a\"b": z.string(),
  "c\\d": z.int(),
  "\u003c/script\u003e\u003cscript\u003ealert(1)\u003c/script\u003e": z.string(),
  "\u0007sino": z.int(),
  "\u202etxt.exe": z.string(),
  "linha\u2028separada": z.int(),
  ["__proto__"]: z.string(),
  constructor: z.int(),
  "${x}": z.string(),
  "`crase`": z.int(),
  "\u200bzero": z.string(),
  "\u0085c1": z.int(),
  ok: z.string(),
});

export type Registro = z.infer<typeof Registro>;
