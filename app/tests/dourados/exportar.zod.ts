import { z } from "zod";

export const Registro = z.discriminatedUnion("tipo", [
  z.object({
    tipo: z.literal("evento"),
    id: z.int(),
    moeda: z.string(),
    msg: z.string(),
    nivel: z.string(),
    duracao: z.number(),
  }),
  z.object({
    tipo: z.literal("pagamento"),
    id: z.int(),
    moeda: z.string(),
    valor: z.string(),
    parcelas: z.int(),
    aprovado: z.boolean(),
    protocolo: z.number(),
  }),
  z.object({
    tipo: z.literal("pedido"),
    id: z.int(),
    moeda: z.string(),
    cliente: z.object({
      nome: z.string(),
      cidade: z.string().nullable(),
    }),
    itens: z.array(z.object({
      sku: z.string(),
      qtd: z.int(),
      preco: z.string(),
    })),
    cupom: z.null(),
    obs: z.string().optional(),
  }),
]);

export type Registro = z.infer<typeof Registro>;
