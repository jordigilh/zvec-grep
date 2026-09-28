export function leaf(value: number): number {
  return value + 1;
}

export function caller(value: number): number {
  return leaf(value);
}

export function chain(value: number): number {
  return caller(value);
}

export class Alpha {
  convert(value: number): number {
    return value + 2;
  }
}

export class Beta {
  convert(value: number): number {
    return value + 3;
  }
}

export function alphaCaller(receiver: Alpha, value: number): number {
  return receiver.convert(value);
}

export function betaCaller(receiver: Beta, value: number): number {
  return receiver.convert(value);
}

export interface Worker {
  work(value: number): number;
}

export function traitCaller(worker: Worker, value: number): number {
  return worker.work(value);
}

export function aliasCaller(value: number): number {
  const fn = leaf;
  return fn(value);
}

export function dynamicCaller(value: number): number {
  const fn: any = leaf;
  return fn(value);
}
