export type Registro =
  | {
    tipo: "evento";
    id: number;
    moeda: string;
    msg: string;
    nivel: string;
    duracao: number;
  }
  | {
    tipo: "pagamento";
    id: number;
    moeda: string;
    valor: string;
    parcelas: number;
    aprovado: boolean;
    protocolo: number;
  }
  | {
    tipo: "pedido";
    id: number;
    moeda: string;
    cliente: {
      nome: string;
      cidade: string | null;
    };
    itens: Array<{
      sku: string;
      qtd: number;
      preco: string;
    }>;
    cupom: null;
    obs?: string;
  };
