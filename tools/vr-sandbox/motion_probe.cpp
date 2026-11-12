// Development capture instrumentation, not installed with the application.
#define wmain unused_probe_main
#include "probe.cpp"
#undef wmain
#include <fstream>
#include <iomanip>
#include <chrono>
#include <thread>

static double seconds(){LARGE_INTEGER c,f;QueryPerformanceCounter(&c);QueryPerformanceFrequency(&f);return double(c.QuadPart)/f.QuadPart;}
static void matrix(std::ostream &out,const vr::HmdMatrix34_t &m){out<<"[";for(int i=0;i<3;++i)for(int j=0;j<4;++j){if(i||j)out<<",";out<<m.m[i][j];}out<<"]";}
static std::string utf8(const std::wstring &s){int n=WideCharToMultiByte(CP_UTF8,0,s.data(),int(s.size()),nullptr,0,nullptr,nullptr);std::string result(n,0);WideCharToMultiByte(CP_UTF8,0,s.data(),int(s.size()),result.data(),n,nullptr,nullptr);return result;}
int wmain(int argc,wchar_t **argv){
    if(argc!=4)return 2;
    const int frames=_wtoi(argv[2]);const double duration=_wtof(argv[3]);
    if(frames<3||frames>80||duration<=0||duration>30)return 2;
    CoInitializeEx(nullptr,COINIT_MULTITHREADED);vr::EVRInitError error;
    auto system=vr::VR_Init(&error,vr::VRApplication_Background);
    if(error!=vr::VRInitError_None){std::cerr<<vr::VR_GetVRInitErrorAsEnglishDescription(error);return 3;}
    ID3D11ShaderResourceView *views[2]={};
    try{
        ComPtr<ID3D11Device> dev;ComPtr<ID3D11DeviceContext> ctx;D3D_FEATURE_LEVEL level;
        check(D3D11CreateDevice(nullptr,D3D_DRIVER_TYPE_HARDWARE,nullptr,0,nullptr,0,D3D11_SDK_VERSION,&dev,&level,&ctx));
        ComPtr<ID3D11Texture2D> textures[2];
        for(int eye=0;eye<2;++eye){auto result=vr::VRCompositor()->GetMirrorTextureD3D11((vr::EVREye)eye,dev.Get(),(void**)&views[eye]);if(result!=vr::VRCompositorError_None)throw std::runtime_error("mirror error "+std::to_string(result));ComPtr<ID3D11Resource> res;views[eye]->GetResource(&res);check(res.As(&textures[eye]));}
        Sleep(250);
        const std::wstring prefix=argv[1];std::ofstream log(prefix+L"-motion.jsonl");if(!log)throw std::runtime_error("cannot create motion manifest");log<<std::setprecision(12);
        std::cout<<"{\"ready\":true}"<<std::endl;
        struct Pending {std::wstring path;Pixels pixels;};
        std::vector<Pending> pending;pending.reserve(frames*2);
        size_t bytes=0;
        const double began=seconds();
        for(int frame=0;frame<frames;++frame){
            double target=began+duration*frame/(frames-1);while(seconds()<target)std::this_thread::sleep_for(std::chrono::milliseconds(1));
            for(int eye=0;eye<2;++eye){
                auto before=seconds();vr::Compositor_FrameTiming timing{};timing.m_nSize=sizeof(timing);vr::VRCompositor()->GetFrameTiming(&timing);
                vr::TrackedDevicePose_t poses[64];system->GetDeviceToAbsoluteTrackingPose(vr::TrackingUniverseStanding,0,poses,64);
                log<<"{\"frame\":"<<frame<<",\"eye\":"<<eye<<",\"before_qpc\":"<<before<<",\"compositor_frame\":"<<timing.m_nFrameIndex<<",\"head\":";matrix(log,poses[0].mDeviceToAbsoluteTracking);
                log<<",\"eyes\":[";for(int k=0;k<2;++k){if(k)log<<",";float l,r,t,b;system->GetProjectionRaw((vr::EVREye)k,&l,&r,&t,&b);log<<"{\"projection\":["<<l<<","<<r<<","<<t<<","<<b<<"],\"eye_to_head\":";matrix(log,system->GetEyeToHeadTransform((vr::EVREye)k));log<<"}";}log<<"],\"hands\":[";
                for(int k=0;k<2;++k){if(k)log<<",";auto index=system->GetTrackedDeviceIndexForControllerRole(k?vr::TrackedControllerRole_RightHand:vr::TrackedControllerRole_LeftHand);if(index>=64||!poses[index].bPoseIsValid)log<<"null";else matrix(log,poses[index].mDeviceToAbsoluteTracking);}log<<"],\"overlays\":[";
                bool first=true;for(auto name:{"CameraPeople","CameraWorld","CameraRecord","CameraQr"}){std::string key=std::string("cc.orbiters.desktop.orbit.")+name;vr::VROverlayHandle_t h;if(vr::VROverlay()->FindOverlay(key.c_str(),&h)!=vr::VROverlayError_None)continue;float width;vr::VROverlay()->GetOverlayWidthInMeters(h,&width);vr::HmdMatrix34_t m;vr::ETrackingUniverseOrigin origin;vr::VROverlay()->GetOverlayTransformAbsolute(h,&origin,&m);if(!first)log<<",";first=false;log<<"{\"key\":"<<std::quoted(key)<<",\"visible\":"<<(vr::VROverlay()->IsOverlayVisible(h)?"true":"false")<<",\"width_m\":"<<width<<",\"transform\":";matrix(log,m);log<<"}";}log<<"]";
                wchar_t suffix[64];swprintf_s(suffix,L"-%03d-%s.png",frame,eye?L"right":L"left");std::wstring path=prefix+suffix;double copied=0;
                auto pixels=read_pixels(dev.Get(),ctx.Get(),textures[eye].Get(),&copied);
                bytes+=pixels.rgba.size();
                if(bytes>size_t(2)*1024*1024*1024)throw std::runtime_error("Capture exceeds 2 GiB raw memory budget; reduce frames");
                pending.push_back({path,std::move(pixels)});
                log<<",\"copied_qpc\":"<<copied<<",\"done_qpc\":"<<seconds()<<",\"path\":"<<std::quoted(utf8(path))<<"}"<<std::endl;
            }
        }
        // PNG encoding is deliberately outside the timed sampling interval.
        // Keep full native pixels, with a bounded raw buffer, then encode them.
        for(auto &item:pending){write_png(item.path.c_str(),item.pixels);std::cout<<std::endl;item.pixels.rgba.clear();item.pixels.rgba.shrink_to_fit();}
        for(auto view:views)if(view)vr::VRCompositor()->ReleaseMirrorTextureD3D11(view);vr::VR_Shutdown();return 0;
    }catch(const std::exception &e){std::cerr<<e.what();for(auto view:views)if(view)vr::VRCompositor()->ReleaseMirrorTextureD3D11(view);vr::VR_Shutdown();return 4;}
}
