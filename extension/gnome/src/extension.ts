import { Extension } from 'resource:///org/gnome/shell/extensions/extension.js';
import { WindowManagerService } from './service.js';

export default class LofiShellExtension extends Extension {
    private service: WindowManagerService | null = null;

    override enable(): void {
        this.service = new WindowManagerService();
        this.service.export();
    }

    override disable(): void {
        if (this.service !== null) {
            this.service.unexport();
            this.service = null;
        }
    }
}
