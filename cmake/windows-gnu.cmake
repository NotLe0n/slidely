# CMake toolchain for whisper.cpp built for windows-gnu (the compilers come from CC/CXX
# in .cargo/config.toml).
# whisper-rs-sys passes MSVC's /utf-8 on every Windows target; GCC takes it for a file.
# Written to the cache, since whisper.cpp's old policies drop a normal variable here.
string(REPLACE "/utf-8" "" _flags "${CMAKE_CXX_FLAGS}")
set(CMAKE_CXX_FLAGS "${_flags}" CACHE STRING "" FORCE)

# ggml drops the "lib" prefix from its libraries on Windows, but rustc only finds static
# libraries named lib*.a when targeting windows-gnu. Undo it whenever it's cleared.
macro(_keep_lib_prefix variable access value)
    if ("${access}" STREQUAL "MODIFIED_ACCESS" AND "${value}" STREQUAL "")
        set(${variable} lib)
    endif()
endmacro()
variable_watch(CMAKE_STATIC_LIBRARY_PREFIX _keep_lib_prefix)
