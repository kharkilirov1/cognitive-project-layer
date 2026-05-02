@Entry
@Component
struct MainPage {
  @State message: string = 'CPL ArkTS fixture';

  build() {
    Column() {
      Text(this.message)
        .fontSize(24)
        .fontWeight(FontWeight.Bold)
    }
    .height('100%')
    .width('100%')
  }
}
