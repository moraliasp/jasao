export type Registro =
  | {
    tipo: "evento";
    id: number;
    moeda: "BRL";
    msg: string;
    nivel: string;
    duracao: number;
  }
  | {
    tipo: "pagamento";
    id: number;
    moeda: "BRL";
    valor: string;
    parcelas: 3;
    aprovado: boolean;
    protocolo: number;
  }
  | {
    tipo: "pedido";
    id: number;
    moeda: "BRL";
    cliente: {
      nome: string;
      cidade: "Cidade Fict\u00edcia" | null;
    };
    itens: Array<{
      sku: string;
      qtd: number;
      preco: string;
    }>;
    cupom: null;
    obs?: "entregar \u00e0 tarde";
  };
