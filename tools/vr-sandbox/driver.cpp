// Developer-only emulated devices. Never linked into the product or smoothing driver.
#define WIN32_LEAN_AND_MEAN
#define NOMINMAX
#include <windows.h>
#include "vendor/openvr_driver.h"
#include <array>
#include <atomic>
#include <chrono>
#include <cmath>
#include <cstdio>
#include <cstring>
#include <string>
using namespace vr;
struct DeviceState { double p[3], q[4]; float inputs[20]; };
struct Shared { uint32_t magic, version, owner, reserved; DeviceState devices[3]; };
static_assert(sizeof(DeviceState)==136);
static_assert(sizeof(Shared)==424);
static Shared snapshot{};
static HANDLE mapping=nullptr, mutex=nullptr, owner=nullptr;
static Shared *shared=nullptr;
static bool alive=false;
static const wchar_t *mapName=L"Local\\OrbitersVrSandbox1112";
static const wchar_t *mutexName=L"Local\\OrbitersVrSandbox1112Mutex";
// Per-eye render size SteamVR is told: 1080x1200 unless %USERPROFILE%/.orbiters-build/vr-sandbox/render-target.txt
// says "<width> <height>". The camera stream uses 2112 2304, which VRChat's 1.5 render scale makes the 3168x3456 eyes
// of Enzo's Quest Pro over Virtual Desktop.
static std::array<uint32_t,2> renderTarget() {
    static std::array<uint32_t,2> size=[]{
        std::array<uint32_t,2> out{1080,1200};
        wchar_t home[MAX_PATH]; DWORD n=GetEnvironmentVariableW(L"USERPROFILE",home,MAX_PATH);
        if(n==0||n>=MAX_PATH)return out;
        std::wstring path=std::wstring(home)+L"\\.orbiters-build\\vr-sandbox\\render-target.txt";
        FILE *f=nullptr; if(_wfopen_s(&f,path.c_str(),L"r")!=0||!f)return out;
        unsigned w=0,h=0; if(fscanf_s(f,"%u %u",&w,&h)==2&&w>=256&&h>=256&&w<=4096&&h<=4096)out={w,h};
        fclose(f); return out;
    }();
    return size;
}
class Device final : public ITrackedDeviceServerDriver, public IVRDisplayComponent, public IVRVirtualDisplay {
    uint32_t slot; TrackedDeviceIndex_t id=k_unTrackedDeviceIndexInvalid;
    std::array<VRInputComponentHandle_t,20> inputs{};
    std::atomic<bool> ready{false};
    std::chrono::steady_clock::time_point epoch=std::chrono::steady_clock::now();
public:
    explicit Device(uint32_t s):slot(s){}
    EVRInitError Activate(uint32_t index) override {
        id=index; auto props=VRProperties(); auto c=props->TrackedDeviceToPropertyContainer(id);
        props->SetStringProperty(c,Prop_TrackingSystemName_String,"orbiters_sandbox");
        props->SetStringProperty(c,Prop_ModelNumber_String,slot?"Quest Pro sandbox controller":"Orbiters sandbox HMD");
        props->SetStringProperty(c,Prop_ManufacturerName_String,"Orbiters developer tools");
        props->SetUint64Property(c,Prop_CurrentUniverseId_Uint64,1112);
        props->SetBoolProperty(c,Prop_DeviceIsWireless_Bool,false);
        props->SetBoolProperty(c,Prop_DeviceProvidesBatteryStatus_Bool,false);
        if(!slot) {
            props->SetFloatProperty(c,Prop_UserIpdMeters_Float,0.064f);
            props->SetFloatProperty(c,Prop_DisplayFrequency_Float,72.f);
            props->SetFloatProperty(c,Prop_SecondsFromVsyncToPhotons_Float,0.0f);
            props->SetBoolProperty(c,Prop_IsOnDesktop_Bool,false);
            props->SetBoolProperty(c,Prop_HasDisplayComponent_Bool,true);
            props->SetBoolProperty(c,Prop_HasVirtualDisplayComponent_Bool,true);
            props->SetBoolProperty(c,Prop_ContainsProximitySensor_Bool,false);
        } else {
            props->SetStringProperty(c,Prop_ControllerType_String,"oculus_touch");
            props->SetStringProperty(c,Prop_InputProfilePath_String,"{oculus}/input/touch_profile.json");
            props->SetStringProperty(c,Prop_RenderModelName_String,slot==1?"oculus_quest_pro_controller_left":"oculus_quest_pro_controller_right");
            props->SetInt32Property(c,Prop_ControllerRoleHint_Int32,slot==1?TrackedControllerRole_LeftHand:TrackedControllerRole_RightHand);
            const char *paths[]={"/input/trigger/value","/input/grip/value","/input/joystick/x","/input/joystick/y","/input/trigger/touch","/input/grip/touch","/input/joystick/click","/input/joystick/touch","/input/a/click","/input/a/touch","/input/b/click","/input/b/touch","/input/x/click","/input/x/touch","/input/y/click","/input/y/touch","/input/system/click","/input/system/touch","/input/thumbrest/touch","/input/trigger/click"};
            for(size_t i=0;i<inputs.size();++i) {
                EVRInputError e;
                if(i<4) e=VRDriverInput()->CreateScalarComponent(c,paths[i],&inputs[i],VRScalarType_Absolute,i>=2?VRScalarUnits_NormalizedTwoSided:VRScalarUnits_NormalizedOneSided);
                else e=VRDriverInput()->CreateBooleanComponent(c,paths[i],&inputs[i]);
                if(e!=VRInputError_None) return VRInitError_Driver_Failed;
            }
            VRInputComponentHandle_t haptic;
            VRDriverInput()->CreateHapticComponent(c,"/output/haptic",&haptic);
        }
        VRDriverLog()->Log(slot==0?"Sandbox HMD activated":slot==1?"Sandbox left controller activated":"Sandbox right controller activated");
        ready.store(true); return VRInitError_None;
    }
    void Deactivate() override { ready.store(false);id=k_unTrackedDeviceIndexInvalid; }
    void EnterStandby() override {}
    void *GetComponent(const char *name) override {
        if(slot)return nullptr;
        if(!strcmp(name,IVRDisplayComponent_Version))return static_cast<IVRDisplayComponent*>(this);
        if(!strcmp(name,IVRVirtualDisplay_Version))return static_cast<IVRVirtualDisplay*>(this);
        return nullptr;
    }
    void Present(const PresentInfo_t *,uint32_t) override {}
    void WaitForPresent() override {
        auto elapsed=std::chrono::duration<double>(std::chrono::steady_clock::now()-epoch).count();
        auto next=(std::floor(elapsed*72)+1)/72;
        Sleep((DWORD)std::max(1.0,(next-elapsed)*1000));
    }
    bool GetTimeSinceLastVsync(float *seconds,uint64_t *counter) override {
        auto elapsed=std::chrono::duration<double>(std::chrono::steady_clock::now()-epoch).count();
        *counter=(uint64_t)(elapsed*72);*seconds=(float)(elapsed-*counter/72.0);return true;
    }
    void DebugRequest(const char*,char *out,uint32_t n) override {if(n) out[0]=0;}
    DriverPose_t GetPose() override {
        DriverPose_t p{}; p.qWorldFromDriverRotation.w=1; p.qDriverFromHeadRotation.w=1;
        auto &s=snapshot.devices[slot];
        p.qRotation={s.q[0],s.q[1],s.q[2],s.q[3]};
        for(int i=0;i<3;++i)p.vecPosition[i]=s.p[i];
        p.poseIsValid=alive; p.deviceIsConnected=alive; p.result=alive?TrackingResult_Running_OK:TrackingResult_Uninitialized;
        return p;
    }
    void frame() {
        if(!ready.load()||id==k_unTrackedDeviceIndexInvalid)return;
        auto pose=GetPose(); VRServerDriverHost()->TrackedDevicePoseUpdated(id,pose,sizeof(pose));
        if(slot)for(size_t i=0;i<inputs.size();++i) {
            float value=alive?snapshot.devices[slot].inputs[i]:0;
            if(i<4)VRDriverInput()->UpdateScalarComponent(inputs[i],value,0);
            else VRDriverInput()->UpdateBooleanComponent(inputs[i],value>0.5f,0);
        }
    }
    void GetWindowBounds(int32_t *x,int32_t *y,uint32_t *w,uint32_t *h) override {*x=0;*y=0;*w=2*renderTarget()[0];*h=renderTarget()[1];}
    bool IsDisplayOnDesktop() override {return false;}
    bool IsDisplayRealDisplay() override {return false;}
    void GetRecommendedRenderTargetSize(uint32_t *w,uint32_t *h) override {*w=renderTarget()[0];*h=renderTarget()[1];}
    void GetEyeOutputViewport(EVREye e,uint32_t *x,uint32_t *y,uint32_t *w,uint32_t *h) override {*x=e==Eye_Left?0:renderTarget()[0];*y=0;*w=renderTarget()[0];*h=renderTarget()[1];}
    void GetProjectionRaw(EVREye e,float *l,float *r,float *t,float *b) override {*l=e==Eye_Left?-1.376f:-.839f;*r=e==Eye_Left?.839f:1.376f;*t=-1.376f;*b=.900f;}
    DistortionCoordinates_t ComputeDistortion(EVREye,float u,float v) override {DistortionCoordinates_t d{};for(int i=0;i<2;++i)d.rfRed[i]=d.rfGreen[i]=d.rfBlue[i]=i?v:u;return d;}
    bool ComputeInverseDistortion(HmdVector2_t *out,EVREye,uint32_t,float u,float v) override {out->v[0]=u;out->v[1]=v;return true;}
};
class Provider final : public IServerTrackedDeviceProvider {
    Device head{0},left{1},right{2};
public:
    EVRInitError Init(IVRDriverContext *context) override {
        VR_INIT_SERVER_DRIVER_CONTEXT(context);
        mapping=OpenFileMappingW(FILE_MAP_READ,FALSE,mapName);
        mutex=OpenMutexW(SYNCHRONIZE|MUTEX_MODIFY_STATE,FALSE,mutexName);
        if(!mapping||!mutex)return VRInitError_Init_HmdNotFound;
        shared=(Shared*)MapViewOfFile(mapping,FILE_MAP_READ,0,0,sizeof(Shared));
        if(!shared||shared->magic!=0x56523132||shared->version!=1)return VRInitError_Init_HmdNotFound;
        owner=OpenProcess(SYNCHRONIZE,FALSE,shared->owner);
        if(!owner||WaitForSingleObject(owner,0)!=WAIT_TIMEOUT)return VRInitError_Init_HmdNotFound;
        snapshot=*shared;alive=true;
        if(!VRServerDriverHost()->TrackedDeviceAdded("orbiters-sandbox-head",TrackedDeviceClass_HMD,&head))return VRInitError_Driver_Failed;
        VRServerDriverHost()->TrackedDeviceAdded("orbiters-sandbox-left",TrackedDeviceClass_Controller,&left);
        VRServerDriverHost()->TrackedDeviceAdded("orbiters-sandbox-right",TrackedDeviceClass_Controller,&right);
        return VRInitError_None;
    }
    void Cleanup() override {alive=false;if(shared)UnmapViewOfFile(shared);for(auto h:{mapping,mutex,owner})if(h)CloseHandle(h);shared=nullptr;mapping=mutex=owner=nullptr;VR_CLEANUP_SERVER_DRIVER_CONTEXT();}
    const char * const *GetInterfaceVersions() override {return k_InterfaceVersions;}
    void RunFrame() override {
        alive=owner&&WaitForSingleObject(owner,0)==WAIT_TIMEOUT;
        if(alive&&WaitForSingleObject(mutex,0)==WAIT_OBJECT_0){snapshot=*shared;ReleaseMutex(mutex);}
        head.frame();left.frame();right.frame();
    }
    bool ShouldBlockStandbyMode() override {return true;}
    void EnterStandby() override {}
    void LeaveStandby() override {}
};
static Provider provider;
extern "C" __declspec(dllexport) void *HmdDriverFactory(const char *name,int *error) {
    if(!strcmp(name,IServerTrackedDeviceProvider_Version))return &provider;
    if(error)*error=VRInitError_Init_InterfaceNotFound;return nullptr;
}
