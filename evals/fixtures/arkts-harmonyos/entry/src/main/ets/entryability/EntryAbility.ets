import UIAbility from '@ohos.app.ability.UIAbility';
import hilog from '@ohos.hilog';

export default class EntryAbility extends UIAbility {
  onCreate(want, launchParam): void {
    hilog.info(0x0000, 'CPLFixture', 'EntryAbility lifecycle bootstrap');
  }

  onWindowStageCreate(windowStage): void {
    windowStage.loadContent('pages/Index');
  }
}
