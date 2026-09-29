import { Helper, make } from './helper';
export { make as exportedMake } from './helper';

export function importedUser(value: Helper) {
    make(value);
}

export function ambiguousCall() {
    load();
}
