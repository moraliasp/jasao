// Documento da aba ativa: todo comando do Rust leva o id dele (ver src-tauri/src/lib.rs).
import { invoke } from "@tauri-apps/api/core";

let ativo: number | null = null;

export const docAtivo = () => ativo;
export const usarDoc = (id: number | null) => void (ativo = id);

/** `invoke` no documento da aba ativa (ou no `id` passado em `args`). */
export const chamar = <T>(cmd: string, args: Record<string, unknown> = {}) => invoke<T>(cmd, { id: ativo, ...args });
