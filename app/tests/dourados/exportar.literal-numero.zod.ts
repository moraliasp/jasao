import { z } from "zod";

export const Registro = z.discriminatedUnion("tipo", [
  z.object({
    tipo: z.literal("evento"),
    id: z.int(),
    moeda: z.literal("BRL"),
    msg: z.string(),
    nivel: z.string(),
    duracao: z.number(),
  }),
  z.object({
    tipo: z.literal("pagamento"),
    id: z.int(),
    moeda: z.literal("BRL"),
    valor: z.string().regex(/^-?(0|[1-9][0-9]*)(\.[0-9]+)?([eE][+-]?[0-9]+)?$/).transform(Number),
    parcelas: z.literal(3),
    aprovado: z.boolean(),
    protocolo: z.number(),
  }),
  z.object({
    tipo: z.literal("pedido"),
    id: z.int(),
    moeda: z.literal("BRL"),
    cliente: z.object({
      nome: z.string(),
      cidade: z.literal("Cidade Fict\u00edcia").nullable(),
    }),
    itens: z.array(z.object({
      sku: z.string(),
      qtd: z.int(),
      preco: z.string().regex(/^-?(0|[1-9][0-9]*)(\.[0-9]+)?([eE][+-]?[0-9]+)?$/).transform(Number),
    })),
    cupom: z.null(),
    obs: z.literal("entregar \u00e0 tarde").optional(),
  }),
]);

export type Registro = z.infer<typeof Registro>;
