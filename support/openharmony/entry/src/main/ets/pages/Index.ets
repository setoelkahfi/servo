import promptAction from '@ohos.promptAction';
import { BusinessError } from '@kit.BasicServicesKit';
import servoshell from 'libservoshell.so';

function prompt_toast(msg: string) {
  promptAction.showToast({
    message: msg,
    duration: 2000
  });
}

// Use the getShared API to obtain the LocalStorage instance shared by stage.
let storage = LocalStorage.getShared()

@Entry(storage)
@Component
struct Index {
  // The state seem to be required to be on the top.
  @State urlToLoad: string = ""
  @State tablist: Array<number> = [0];
  @State currentIndex: number = 0;
  private nextTabId: number = 1;

  reportCommandError(error: BusinessError): void {
    console.error(`Browser command failed: ${error.message}`);
    prompt_toast(this.getUIContext().getHostContext()!.resourceManager.getStringSync(
      $r('app.string.browser_unavailable').id));
  }

  @Builder tabBuilder(id: number) {
    Row() {
      Text(String(id + 1))
        .layoutWeight(1)
        .maxLines(1)
      Button('X')
      .backgroundColor('#00000000')
      .width(40)
      .height(40)
      .visibility(id != 0 ? Visibility.Visible: Visibility.Hidden)
      .fontColor(Color.Black)
      .fontWeight(FontWeight.Bolder)
      .onClick((event) => {
          try {
            const index = this.tablist.indexOf(id);
            servoshell.deleteWebview(index, this.tablist);
            this.currentIndex = 0;
            this.tablist.splice(index,1);
          } catch (error) {
            this.reportCommandError(error as BusinessError);
          }
        })
    }
    .width(96)
    .height(48)
    .padding({ left: 8, right: 4 })
    .justifyContent(FlexAlign.Center)
    .backgroundColor(this.tablist[this.currentIndex] === id ? '#1B8381': Color.White)
    .borderRadius(10)
  }

  aboutToAppear(): void {
    console.info("aboutToAppear - registering callbacks!")
    servoshell.registerURLcallback((new_url: string) => {
      console.info('New URL from native: ', new_url)
      this.urlToLoad = new_url
    })
    servoshell.registerPromptToastCallback(prompt_toast)
  }

  // Called when the user swipes from the right or left edge to the middle
  // Default behavior is bringing the app to the background.
  onBackPress(): boolean {
    try {
      return servoshell.goBack();
    } catch (error) {
      this.reportCommandError(error as BusinessError);
      return false;
    }
  }

  build() {
    Column() {
      Row() {
        Button('+')
          .backgroundColor(Color.White)
          .fontColor(Color.Black)
          .fontWeight(FontWeight.Bolder)
          .fontSize(22)
          .width(40)
          .height(48)
          .onClick((event) => {
            const new_id = this.nextTabId++;
            // the order is important as setting the new index will automatically create a new surface.
            servoshell.nextWindowId(new_id);
            this.currentIndex = this.tablist.push(new_id) - 1;
          })
        Button('⇦')
          .backgroundColor(Color.White)
          .fontColor(Color.Black)
          .fontWeight(FontWeight.Bolder)
          .width(40)
          .height(48)
          .fontSize(12)
          .onClick(() => {
            this.onBackPress()
          })
        Button('⇨')
          .backgroundColor(Color.White)
          .fontColor(Color.Black)
          .fontWeight(FontWeight.Bolder)
          .fontSize(12)
          .width(40)
          .height(48)
          .onClick(() => {
            try {
              servoshell.goForward()
            } catch (error) {
              this.reportCommandError(error as BusinessError);
            }
          })
        TextInput({ placeholder: 'URL', text: $$this.urlToLoad })
          .type(InputType.URL)
          .layoutWeight(1)
          .height(48)
          .onChange((value) => {
            this.urlToLoad = value
          })
          .onSubmit((EnterKeyType) => {
            try {
              servoshell.loadURL(this.urlToLoad)
            } catch (error) {
              this.reportCommandError(error as BusinessError);
            }
          })
      }
      .width('100%')
      .height(48)
      .flexShrink(0)

      Tabs({ barPosition: BarPosition.Start, index: this.currentIndex}) {
        ForEach(this.tablist, (item: number) => {
          TabContent() {
            XComponent({
              id: `Servo${item}`,
              type: XComponentType.SURFACE,
              libraryname: 'servoshell'
            })
              .width('100%')
              .height('100%')
              .focusable(true)
              .focusOnTouch(true)
          }.tabBar(this.tabBuilder(item))
        }, (item: number) => String(item))
      }.barHeight(48)
      .barMode(BarMode.Scrollable)
      .layoutWeight(1)
      .scrollable(false)
      .onChange((index: number) => {
        this.currentIndex = index;
        try {
          servoshell.focusWebview(index, this.tablist);
        } catch (error) {
          this.reportCommandError(error as BusinessError);
        }
      })
      .backgroundColor('#e1e1e1')
    }
    .width('100%')
    .height('100%')
  }
}
