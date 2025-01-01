

export interface GolemService {
    getComponent(): Promise<any>;
    createComponent(): Promise<any>;
    getComponentById(): Promise<any>;
}