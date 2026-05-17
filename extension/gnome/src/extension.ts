import { Extension } from 'resource:///org/gnome/shell/extensions/extension.js';
import { LauncherAnimationSuppressor } from './launcher.js';
import { WindowManagerService } from './service.js';

export default class LofiShellExtension extends Extension {
    private service: WindowManagerService | null = null;
    private animations: LauncherAnimationSuppressor | null = null;

    override enable(): void {
        this.service = new WindowManagerService();
        this.service.export();
        this.animations = new LauncherAnimationSuppressor();
        this.animations.enable();
    }

    override disable(): void {
        if (this.animations !== null) {
            this.animations.disable();
            this.animations = null;
        }
        if (this.service !== null) {
            this.service.unexport();
            this.service = null;
        }
    }
}
