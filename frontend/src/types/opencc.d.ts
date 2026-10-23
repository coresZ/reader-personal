declare module '*chinese.js' {
  /** 简体 → 繁体（legado 词表）。 */
  export function traditionalized(text: string): string;
  /** 繁体 → 简体（legado 词表）。 */
  export function simplified(text: string): string;
}

/** 由 scripts/build-legado-chinese-dicts.mjs 生成的 opencc-js 序列化词典。 */
declare module '*legadoT2sDict.js' {
  const dict: string;
  export default dict;
}

declare module '*legadoS2tDict.js' {
  const dict: string;
  export default dict;
}
