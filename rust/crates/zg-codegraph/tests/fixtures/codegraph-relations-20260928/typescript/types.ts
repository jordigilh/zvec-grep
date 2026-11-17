export class Base {}

export interface Contract {}

export class Child extends Base implements Contract {
    value: Base;
}
